use crate::{
    locks::*,
    projection::{ProjectR, ProjectW},
    synchronizer::{
        SynchronizerMetadata, SynchronizerReadLock, SynchronizerSized, SynchronizerType,
        SynchronizerUnsized, SynchronizerWriteLock,
    },
};
use std::{
    fmt::Debug,
    marker::PhantomData,
    sync::{Arc, OnceLock},
};

/// Specifies the underlying mutable synchronization primitive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ImplementationMut {
    RwLock,
    Mutex,
    Rcu,
}

/// Determines what should happen if the underlying synchronization primitive is poisoned by being held during
/// a `panic!`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PoisonPolicy {
    /// Ignore poisoned values.
    Ignore,
    /// Panic if the value is poisoned.
    Panic,
}

type Poison = std::sync::atomic::AtomicBool;

pub struct LockMetadata {
    pub poison: Option<Poison>,
    /// Process-unique identity for lock-order tracking. Shared by all clones
    /// and projections of the same container.
    #[cfg(feature = "deadlock_detection")]
    pub(crate) lock_id: crate::lockdep::LockId,
}

impl LockMetadata {
    pub const fn poison_panic() -> Self {
        Self::poison_policy(PoisonPolicy::Panic)
    }

    pub const fn poison_ignore() -> Self {
        Self::poison_policy(PoisonPolicy::Ignore)
    }

    pub const fn poison_policy(policy: PoisonPolicy) -> Self {
        match policy {
            PoisonPolicy::Ignore => Self {
                poison: None,
                #[cfg(feature = "deadlock_detection")]
                lock_id: crate::lockdep::LockId::unassigned(),
            },
            PoisonPolicy::Panic => Self {
                poison: Some(Poison::new(false)),
                #[cfg(feature = "deadlock_detection")]
                lock_id: crate::lockdep::LockId::unassigned(),
            },
        }
    }
}

#[allow(clippy::type_complexity)]
pub enum SharedImpl<T: ?Sized> {
    Value(SynchronizerUnsized<LockMetadata, T>),
    Box(SynchronizerUnsized<LockMetadata, Box<T>>),
    // Lazy(Arc<(OnceCell<Synchronizer<LockMetadata, T>>, fn() -> T)>),
    Projection(Arc<dyn SharedMutProjection<T> + 'static>),
    ProjectionRO(Arc<dyn SharedProjection<T> + 'static>),
    ProjectionStatic(*const dyn SharedMutProjection<T>),
    ProjectionStaticRO(*const dyn SharedProjection<T>),
}

// UNSAFETY: The construction and projection of SharedImpl requires Send + Sync, so we can guarantee that
// all instances of SharedImpl are Send + Sync.
unsafe impl<T: ?Sized> Send for SharedImpl<T> {}
unsafe impl<T: ?Sized> Sync for SharedImpl<T> {}

impl<T: ?Sized + Debug> Debug for SharedImpl<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SharedImpl::Value(lock) => lock.fmt(f),
            SharedImpl::Box(lock) => lock.fmt(f),
            // TODO: We should format the underlying projection
            SharedImpl::Projection(_)
            | SharedImpl::ProjectionRO(_)
            | SharedImpl::ProjectionStatic(_)
            | SharedImpl::ProjectionStaticRO(_) => f.write_fmt(format_args!("(projection)")),
        }
    }
}

impl<T: ?Sized> Clone for SharedImpl<T> {
    fn clone(&self) -> Self {
        match &self {
            SharedImpl::Value(lock) => SharedImpl::Value(lock.clone()),
            SharedImpl::Box(lock) => SharedImpl::Box(lock.clone()),
            SharedImpl::Projection(x) => SharedImpl::Projection(x.clone()),
            SharedImpl::ProjectionRO(x) => SharedImpl::ProjectionRO(x.clone()),
            SharedImpl::ProjectionStatic(x) => SharedImpl::ProjectionStatic(*x),
            SharedImpl::ProjectionStaticRO(x) => SharedImpl::ProjectionStaticRO(*x),
        }
    }
}

/// True when a panic is unwinding for a reason that should poison a held lock.
///
/// A genuine panic while holding a lock may have left the guarded data half-written, so poisoning
/// is correct. A lockdep-triggered panic, however, aborts *before* acquiring the second lock — the
/// data under every currently-held lock is consistent — so it must not poison, otherwise a single
/// proactive deadlock warning would cascade into a poison outage across unrelated requests.
pub(crate) fn panicking_should_poison() -> bool {
    if !std::thread::panicking() {
        return false;
    }
    #[cfg(feature = "deadlock_detection")]
    {
        !crate::lockdep::is_aborting()
    }
    #[cfg(not(feature = "deadlock_detection"))]
    {
        true
    }
}

/// Marks a lock's metadata poisoned if dropped during a panic. This mirrors what the read/write
/// guards do, for the fast `set`/`try_set` paths that mutate the value without constructing a guard.
struct PoisonOnPanic<'a>(&'a LockMetadata);

impl Drop for PoisonOnPanic<'_> {
    fn drop(&mut self) {
        if let Some(poison) = &self.0.poison {
            if panicking_should_poison() {
                poison.store(true, std::sync::atomic::Ordering::Release);
            }
        }
    }
}

impl<T> SharedImpl<T> {
    /// Attempt to unwrap this synchronized object if we are the only holder of its value.
    pub fn try_unwrap(self) -> Result<T, Self> {
        self.check_poison();
        match self {
            SharedImpl::Value(lock) => lock.try_unwrap().map_err(SharedImpl::Value),
            SharedImpl::Box(lock) => lock.try_unwrap().map_err(SharedImpl::Box).map(|x| *x),
            SharedImpl::Projection(_) => Err(self),
            SharedImpl::ProjectionRO(_) => Err(self),
            SharedImpl::ProjectionStatic(_) => Err(self),
            SharedImpl::ProjectionStaticRO(_) => Err(self),
        }
    }

    /// Replace the whole value.
    ///
    /// For a directly-held value this replaces it at the primitive level, which lets RCU skip the
    /// clone that acquiring a write guard would otherwise perform. Projections must preserve the
    /// rest of the root value, so they fall back to acquiring a write lock and assigning.
    pub fn set(&self, value: T) {
        match self {
            SharedImpl::Value(lock) => {
                self.check_poison();
                #[cfg(feature = "deadlock_detection")]
                crate::lockdep::check_blocking_acquire(
                    lock.metadata(),
                    lock.sync_type(),
                    true,
                    std::any::type_name::<T>(),
                );
                let _poison = PoisonOnPanic(lock.metadata());
                lock.set(value);
            }
            _ => {
                *self.lock_write() = value;
            }
        }
    }

    /// Try to replace the whole value without blocking, returning it back on failure.
    pub fn try_set(&self, value: T) -> Result<(), T> {
        match self {
            SharedImpl::Value(lock) => {
                self.check_poison();
                let _poison = PoisonOnPanic(lock.metadata());
                lock.try_set(value)
            }
            _ => match self.try_lock_write() {
                Some(mut guard) => {
                    *guard = value;
                    Ok(())
                }
                None => Err(value),
            },
        }
    }
}

impl<T: ?Sized> SharedImpl<T> {
    fn check_poison(&self) {
        let metadata = match &self {
            SharedImpl::Value(lock) => Some(lock.metadata()),
            SharedImpl::Box(lock) => Some(lock.metadata()),
            _ => None,
        };
        if let Some(metadata) = metadata {
            if let Some(poison) = &metadata.poison {
                if poison.load(std::sync::atomic::Ordering::Acquire) {
                    panic!("This lock was poisoned by a panic elsewhere in the code.");
                }
            }
        }
    }

    pub fn try_unwrap_box(self) -> Result<Box<T>, Self> {
        self.check_poison();
        match self {
            // A `Value` container stores `T` inline; an unsized `T` cannot be
            // moved out of it, so only `Box` containers can unwrap here.
            SharedImpl::Value(_) => Err(self),
            SharedImpl::Box(lock) => lock.try_unwrap().map_err(SharedImpl::Box),
            SharedImpl::Projection(_) => Err(self),
            SharedImpl::ProjectionRO(_) => Err(self),
            SharedImpl::ProjectionStatic(_) => Err(self),
            SharedImpl::ProjectionStaticRO(_) => Err(self),
        }
    }

    pub fn lock_read(&self) -> SharedReadLock<T> {
        self.check_poison();
        match &self {
            SharedImpl::Value(lock) => {
                #[cfg(feature = "deadlock_detection")]
                crate::lockdep::check_blocking_acquire(
                    lock.metadata(),
                    lock.sync_type(),
                    false,
                    std::any::type_name::<T>(),
                );
                lock.lock_read().into()
            }
            SharedImpl::Box(lock) => {
                #[cfg(feature = "deadlock_detection")]
                crate::lockdep::check_blocking_acquire(
                    lock.metadata(),
                    lock.sync_type(),
                    false,
                    std::any::type_name::<T>(),
                );
                lock.lock_read().into()
            }
            SharedImpl::Projection(p) => p.lock_read(),
            SharedImpl::ProjectionRO(p) => p.read(),
            SharedImpl::ProjectionStatic(p) => unsafe { p.as_ref().unwrap_unchecked() }.lock_read(),
            SharedImpl::ProjectionStaticRO(p) => unsafe { p.as_ref().unwrap_unchecked() }.read(),
        }
    }

    pub fn try_lock_read(&self) -> Option<SharedReadLock<T>> {
        self.check_poison();
        match &self {
            SharedImpl::Value(lock) => lock.try_lock_read().map(|x| x.into()),
            SharedImpl::Box(lock) => lock.try_lock_read().map(|x| x.into()),
            SharedImpl::Projection(p) => p.try_lock_read(),
            SharedImpl::ProjectionRO(p) => p.try_lock_read(),
            SharedImpl::ProjectionStatic(p) => {
                unsafe { p.as_ref().unwrap_unchecked() }.try_lock_read()
            }
            SharedImpl::ProjectionStaticRO(p) => {
                unsafe { p.as_ref().unwrap_unchecked() }.try_lock_read()
            }
        }
    }

    #[allow(unused)]
    pub(crate) fn lock_read_owned(&self) -> SharedReadLockOwned<T> {
        let container = self.clone();
        // UNSAFETY: We are keeping the SharedReadLock with the lock it is taken from, allowing us to safely transmute this lifetime to 'static.
        let inner = unsafe {
            std::mem::transmute::<SharedReadLock<'_, T>, SharedReadLock<'static, T>>(
                container.lock_read(),
            )
        };
        SharedReadLockOwned { inner, container }
    }

    pub fn lock_write(&self) -> SharedWriteLock<T> {
        self.check_poison();
        match &self {
            SharedImpl::Value(lock) => {
                #[cfg(feature = "deadlock_detection")]
                crate::lockdep::check_blocking_acquire(
                    lock.metadata(),
                    lock.sync_type(),
                    true,
                    std::any::type_name::<T>(),
                );
                lock.lock_write().into()
            }
            SharedImpl::Box(lock) => {
                #[cfg(feature = "deadlock_detection")]
                crate::lockdep::check_blocking_acquire(
                    lock.metadata(),
                    lock.sync_type(),
                    true,
                    std::any::type_name::<T>(),
                );
                lock.lock_write().into()
            }
            SharedImpl::Projection(p) => p.lock_write(),
            SharedImpl::ProjectionRO(_) => unreachable!("This should not be possible"),
            SharedImpl::ProjectionStatic(p) => {
                unsafe { p.as_ref().unwrap_unchecked() }.lock_write()
            }
            SharedImpl::ProjectionStaticRO(_) => unreachable!("This should not be possible"),
        }
    }

    pub fn try_lock_write(&self) -> Option<SharedWriteLock<T>> {
        self.check_poison();
        match &self {
            SharedImpl::Value(lock) => lock.try_lock_write().map(|x| x.into()),
            SharedImpl::Box(lock) => lock.try_lock_write().map(|x| x.into()),
            SharedImpl::Projection(p) => p.try_lock_write(),
            SharedImpl::ProjectionRO(_) => unreachable!("This should not be possible"),
            SharedImpl::ProjectionStatic(p) => {
                unsafe { p.as_ref().unwrap_unchecked() }.try_lock_write()
            }
            SharedImpl::ProjectionStaticRO(_) => unreachable!("This should not be possible"),
        }
    }

    #[allow(unused)]
    pub(crate) fn lock_write_owned(&self) -> SharedWriteLockOwned<T> {
        let container = self.clone();
        // UNSAFETY: We are keeping the SharedReadLock with the lock it is taken from, allowing us to safely transmute this lifetime to 'static.
        let inner = unsafe {
            std::mem::transmute::<SharedWriteLock<'_, T>, SharedWriteLock<'static, T>>(
                container.lock_write(),
            )
        };
        SharedWriteLockOwned { inner, container }
    }
}

pub enum SharedGlobalImpl<T> {
    Value(SynchronizerSized<LockMetadata, T>),
    Box(SynchronizerSized<LockMetadata, Box<T>>),
    Lazy(
        OnceLock<SynchronizerSized<LockMetadata, T>>,
        fn() -> T,
        SynchronizerType,
        PoisonPolicy,
    ),
}

#[repr(transparent)]
pub struct SharedGlobalProjection<T>(SharedGlobalImpl<T>);

// UNSAFETY: We cannot construct a SharedGlobalImpl that isn't safe to send across thread boundards, so we force this be to Send + Sync
unsafe impl<T: Send> Send for SharedGlobalImpl<T> {}
unsafe impl<T: Send> Sync for SharedGlobalImpl<T> {}

impl<T: Send> SharedGlobalImpl<T> {}

impl<T: Send> SharedGlobalImpl<T> {
    pub(crate) const fn lazy(
        f: fn() -> T,
        sync_type: SynchronizerType,
        policy: PoisonPolicy,
    ) -> Self {
        Self::Lazy(OnceLock::new(), f, sync_type, policy)
    }

    pub(crate) fn read(&self) -> SharedReadLock<T> {
        let lock = match self {
            SharedGlobalImpl::Value(lock) => lock,
            SharedGlobalImpl::Box(lock) => return Self::read_sized(lock).into(),
            SharedGlobalImpl::Lazy(lock, callback, sync_type, policy) => lock.get_or_init(|| {
                SynchronizerSized::new(
                    LockMetadata::poison_policy(*policy),
                    *sync_type,
                    (callback)(),
                )
            }),
        };
        Self::read_sized(lock).into()
    }

    fn read_sized<U>(
        lock: &SynchronizerSized<LockMetadata, U>,
    ) -> SynchronizerReadLock<'_, LockMetadata, U> {
        #[cfg(feature = "deadlock_detection")]
        crate::lockdep::check_blocking_acquire(
            lock.metadata(),
            lock.sync_type(),
            false,
            std::any::type_name::<U>(),
        );
        lock.lock_read()
    }

    pub(crate) fn write(&self) -> SharedWriteLock<T> {
        let lock = match self {
            SharedGlobalImpl::Value(lock) => lock,
            SharedGlobalImpl::Box(lock) => return Self::write_sized(lock).into(),
            SharedGlobalImpl::Lazy(lock, callback, sync_type, policy) => lock.get_or_init(|| {
                SynchronizerSized::new(
                    LockMetadata::poison_policy(*policy),
                    *sync_type,
                    (callback)(),
                )
            }),
        };
        Self::write_sized(lock).into()
    }

    fn write_sized<U>(
        lock: &SynchronizerSized<LockMetadata, U>,
    ) -> SynchronizerWriteLock<'_, LockMetadata, U> {
        #[cfg(feature = "deadlock_detection")]
        crate::lockdep::check_blocking_acquire(
            lock.metadata(),
            lock.sync_type(),
            true,
            std::any::type_name::<U>(),
        );
        lock.lock_write()
    }

    pub(crate) fn try_lock_read(&self) -> Option<SharedReadLock<T>> {
        match self {
            SharedGlobalImpl::Value(lock) => lock.try_lock_read().map(|x| x.into()),
            SharedGlobalImpl::Box(lock) => lock.try_lock_read().map(|x| x.into()),
            SharedGlobalImpl::Lazy(lock, callback, sync_type, policy) => lock
                .get_or_init(|| {
                    SynchronizerSized::new(
                        LockMetadata::poison_policy(*policy),
                        *sync_type,
                        (callback)(),
                    )
                })
                .try_lock_read()
                .map(|x| x.into()),
        }
    }

    pub(crate) fn try_lock_write(&self) -> Option<SharedWriteLock<T>> {
        match self {
            SharedGlobalImpl::Value(lock) => lock.try_lock_write().map(|x| x.into()),
            SharedGlobalImpl::Box(lock) => lock.try_lock_write().map(|x| x.into()),
            SharedGlobalImpl::Lazy(lock, callback, sync_type, policy) => lock
                .get_or_init(|| {
                    SynchronizerSized::new(
                        LockMetadata::poison_policy(*policy),
                        *sync_type,
                        (callback)(),
                    )
                })
                .try_lock_write()
                .map(|x| x.into()),
        }
    }
}

impl<T: Send> SharedProjection<T> for SharedGlobalImpl<T> {
    fn read(&self) -> SharedReadLock<T> {
        (*self).read()
    }

    fn try_lock_read(&self) -> Option<SharedReadLock<T>> {
        (*self).try_lock_read()
    }
}

impl<U: ?Sized, T: Send> SharedProjection<U> for SharedGlobalProjection<T>
where
    (): ProjectR<T, U>,
{
    fn read(&self) -> SharedReadLock<U> {
        struct HiddenLock<'a, T: ?Sized, P: ?Sized> {
            lock: SharedReadLock<'a, T>,
            _marker: PhantomData<P>,
        }

        impl<'a, T: ?Sized, P: ?Sized> std::ops::Deref for HiddenLock<'a, T, P>
        where
            (): ProjectR<T, P>,
        {
            type Target = P;
            fn deref(&self) -> &Self::Target {
                ().project(&*self.lock)
            }
        }

        let lock = HiddenLock {
            lock: self.0.read(),
            _marker: PhantomData,
        };

        SharedReadLock::from_projection(Box::new(lock))
    }

    fn try_lock_read(&self) -> Option<SharedReadLock<U>> {
        struct HiddenLock<'a, T: ?Sized, P: ?Sized> {
            lock: SharedReadLock<'a, T>,
            _marker: PhantomData<P>,
        }

        impl<'a, T: ?Sized, P: ?Sized> std::ops::Deref for HiddenLock<'a, T, P>
        where
            (): ProjectR<T, P>,
        {
            type Target = P;
            fn deref(&self) -> &Self::Target {
                ().project(&*self.lock)
            }
        }

        let Some(lock) = self.0.try_lock_read() else {
            return None;
        };

        let lock = HiddenLock {
            lock,
            _marker: PhantomData,
        };

        Some(SharedReadLock::from_projection(Box::new(lock)))
    }
}

impl<T: Send> SharedMutProjection<T> for SharedGlobalImpl<T> {
    fn lock_read(&self) -> SharedReadLock<T> {
        (*self).read()
    }

    fn lock_write(&self) -> SharedWriteLock<T> {
        (*self).write()
    }

    fn try_lock_read(&self) -> Option<SharedReadLock<T>> {
        (*self).try_lock_read()
    }

    fn try_lock_write(&self) -> Option<SharedWriteLock<T>> {
        (*self).try_lock_write()
    }
}

impl<U: ?Sized, T: Send> SharedMutProjection<U> for SharedGlobalProjection<T>
where
    (): ProjectR<T, U> + ProjectW<T, U>,
{
    fn lock_read(&self) -> SharedReadLock<U> {
        struct HiddenLock<'a, T: ?Sized, P: ?Sized> {
            lock: SharedReadLock<'a, T>,
            _marker: PhantomData<P>,
        }

        impl<'a, T: ?Sized, P: ?Sized> std::ops::Deref for HiddenLock<'a, T, P>
        where
            (): ProjectR<T, P>,
        {
            type Target = P;
            fn deref(&self) -> &Self::Target {
                ().project(&*self.lock)
            }
        }

        let lock = HiddenLock {
            lock: self.0.read(),
            _marker: PhantomData,
        };

        SharedReadLock::from_projection(Box::new(lock))
    }

    fn try_lock_read(&self) -> Option<SharedReadLock<U>> {
        struct HiddenLock<'a, T: ?Sized, P: ?Sized> {
            lock: SharedReadLock<'a, T>,
            _marker: PhantomData<P>,
        }

        impl<'a, T: ?Sized, P: ?Sized> std::ops::Deref for HiddenLock<'a, T, P>
        where
            (): ProjectR<T, P>,
        {
            type Target = P;
            fn deref(&self) -> &Self::Target {
                ().project(&*self.lock)
            }
        }

        let Some(lock) = self.0.try_lock_read() else {
            return None;
        };

        let lock = HiddenLock {
            lock,
            _marker: PhantomData,
        };

        Some(SharedReadLock::from_projection(Box::new(lock)))
    }

    fn lock_write(&self) -> SharedWriteLock<U> {
        struct HiddenLock<'a, T: ?Sized, P: ?Sized> {
            lock: SharedWriteLock<'a, T>,
            _marker: PhantomData<P>,
        }

        impl<'a, T: ?Sized, P: ?Sized> std::ops::Deref for HiddenLock<'a, T, P>
        where
            (): ProjectR<T, P>,
        {
            type Target = P;
            fn deref(&self) -> &Self::Target {
                ().project(&*self.lock)
            }
        }

        impl<'a, T: ?Sized, P: ?Sized> std::ops::DerefMut for HiddenLock<'a, T, P>
        where
            (): ProjectR<T, P> + ProjectW<T, P>,
        {
            fn deref_mut(&mut self) -> &mut Self::Target {
                ().project_mut(&mut *self.lock)
            }
        }

        let lock = HiddenLock {
            lock: self.0.write(),
            _marker: PhantomData,
        };

        SharedWriteLock::from_projection(Box::new(lock))
    }

    fn try_lock_write(&self) -> Option<SharedWriteLock<U>> {
        struct HiddenLock<'a, T: ?Sized, P: ?Sized> {
            lock: SharedWriteLock<'a, T>,
            _marker: PhantomData<P>,
        }

        impl<'a, T: ?Sized, P: ?Sized> std::ops::Deref for HiddenLock<'a, T, P>
        where
            (): ProjectR<T, P>,
        {
            type Target = P;
            fn deref(&self) -> &Self::Target {
                ().project(&*self.lock)
            }
        }

        impl<'a, T: ?Sized, P: ?Sized> std::ops::DerefMut for HiddenLock<'a, T, P>
        where
            (): ProjectR<T, P> + ProjectW<T, P>,
        {
            fn deref_mut(&mut self) -> &mut Self::Target {
                ().project_mut(&mut *self.lock)
            }
        }

        let Some(lock) = self.0.try_lock_write() else {
            return None;
        };

        let lock = HiddenLock {
            lock,
            _marker: PhantomData,
        };

        Some(SharedWriteLock::from_projection(Box::new(lock)))
    }
}

pub trait SharedMutProjection<T: ?Sized>: Send + Sync {
    fn lock_read(&self) -> SharedReadLock<T>;
    fn try_lock_read(&self) -> Option<SharedReadLock<T>>;
    fn lock_write(&self) -> SharedWriteLock<T>;
    fn try_lock_write(&self) -> Option<SharedWriteLock<T>>;
}

pub trait SharedProjection<T: ?Sized>: Send + Sync {
    fn read(&self) -> SharedReadLock<T>;
    fn try_lock_read(&self) -> Option<SharedReadLock<T>>;
}

impl<T: ?Sized> SharedProjection<T> for dyn SharedMutProjection<T> {
    fn read(&self) -> SharedReadLock<T> {
        self.lock_read()
    }

    fn try_lock_read(&self) -> Option<SharedReadLock<T>> {
        SharedMutProjection::try_lock_read(self)
    }
}
