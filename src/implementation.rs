use crate::locks::*;
use once_cell::sync::OnceCell;
use parking_lot::{Mutex, RwLock};
use std::{
    fmt::Debug,
    marker::PhantomData,
    sync::{atomic::AtomicBool, Arc},
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

impl PoisonPolicy {
    fn check<T>(&self, poison: &AtomicBool, res: T) -> T {
        if *self == PoisonPolicy::Panic && poison.load(std::sync::atomic::Ordering::Acquire) {
            panic!("This lock was poisoned by a panic elsewhere in the code.");
        }
        res
    }

    fn get_poison<'a>(&self, poison: &'a AtomicBool) -> Option<&'a AtomicBool> {
        if *self == PoisonPolicy::Panic {
            Some(poison)
        } else {
            None
        }
    }
}

type Poison = std::sync::atomic::AtomicBool;

#[allow(clippy::type_complexity)]
pub enum SharedImpl<T: ?Sized> {
    /// Only usable by non-mutable shares.
    Arc(Arc<SharedImplArc<T>>),
    ArcBox(Arc<SharedImplArcBox<T>>),
    /// RCU-mode, which requires us to bring a cloning function along for the ride.
    ReadCopyUpdate(Arc<SharedImplRCU<T>>),
    RwLock(Arc<SharedImplPoisonable<T, RwLock<T>>>),
    /// Used for unsized types
    RwLockBox(Arc<SharedImplPoisonable<T, RwLock<Box<T>>>>),
    Mutex(Arc<SharedImplPoisonable<T, Mutex<T>>>),
    Projection(Arc<dyn SharedMutProjection<T> + 'static>),
    ProjectionRO(Arc<dyn SharedProjection<T> + 'static>),
}

pub trait SharedImplOps<T: ?Sized>: SharedLockOps<T> {
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized;

    fn get_poison(&self) -> Option<(PoisonPolicy, &Poison)> {
        None
    }

    fn lock_read_impl(&self) -> SharedReadLock<T> {
        let inner = self.lock_read();
        if let Some((policy, poison)) = self.get_poison() {
            SharedReadLock {
                inner: policy.check(poison, inner),
                poison: Some(poison),
            }
        } else {
            SharedReadLock {
                inner,
                poison: None,
            }
        }
    }

    fn try_lock_read_impl(&self) -> Option<SharedReadLock<T>> {
        let inner = self.try_lock_read();
        inner.map(|inner| {
            if let Some((policy, poison)) = self.get_poison() {
                SharedReadLock {
                    inner: policy.check(poison, inner),
                    poison: Some(poison),
                }
            } else {
                SharedReadLock {
                    inner,
                    poison: None,
                }
            }
        })
    }

    fn lock_write_impl(&self) -> SharedWriteLock<T> {
        let inner = self.lock_write();
        if let Some((policy, poison)) = self.get_poison() {
            SharedWriteLock {
                inner: policy.check(poison, inner),
                poison: Some(poison),
            }
        } else {
            SharedWriteLock {
                inner,
                poison: None,
            }
        }
    }

    fn try_lock_write_impl(&self) -> Option<SharedWriteLock<T>> {
        let inner = self.try_lock_write();
        inner.map(|inner| {
            if let Some((policy, poison)) = self.get_poison() {
                SharedWriteLock {
                    inner: policy.check(poison, inner),
                    poison: Some(poison),
                }
            } else {
                SharedWriteLock {
                    inner,
                    poison: None,
                }
            }
        })
    }
}

pub trait SharedLockOps<T: ?Sized> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug;
    fn lock_read(&self) -> SharedReadLockInner<T>;
    fn try_lock_read(&self) -> Option<SharedReadLockInner<T>>;
    fn lock_write(&self) -> SharedWriteLockInner<T>;
    fn try_lock_write(&self) -> Option<SharedWriteLockInner<T>>;
}

pub const fn make_shared_rcu<T>(cloner: fn(&Arc<T>) -> Box<T>, t: Arc<T>) -> SharedImplRCU<T> {
    SharedImplRCU {
        cloner,
        lock: RwLock::new(t),
    }
}

pub const fn make_shared_arc<T>(value: T) -> SharedImplArc<T> {
    SharedImplArc { value }
}

pub const fn make_shared_arc_box<T: ?Sized>(value: Box<T>) -> SharedImplArcBox<T> {
    SharedImplArcBox { value }
}

pub const fn make_shared_mutex<T>(policy: PoisonPolicy, t: T) -> SharedImplPoisonable<T, Mutex<T>> {
    SharedImplPoisonable {
        policy,
        poison: AtomicBool::new(false),
        container: Mutex::new(t),
        _unused: PhantomData {},
    }
}

pub const fn make_shared_rwlock<T>(
    policy: PoisonPolicy,
    t: T,
) -> SharedImplPoisonable<T, RwLock<T>> {
    SharedImplPoisonable {
        policy,
        poison: AtomicBool::new(false),
        container: RwLock::new(t),
        _unused: PhantomData {},
    }
}

pub const fn make_shared_rwlock_box<T: ?Sized>(
    policy: PoisonPolicy,
    t: Box<T>,
) -> SharedImplPoisonable<T, RwLock<Box<T>>> {
    SharedImplPoisonable {
        policy,
        poison: AtomicBool::new(false),
        container: RwLock::new(t),
        _unused: PhantomData {},
    }
}

pub struct SharedImplArc<T: ?Sized> {
    value: T,
}

pub struct SharedImplArcBox<T: ?Sized> {
    value: Box<T>,
}

pub struct SharedImplRCU<T: ?Sized> {
    cloner: fn(&Arc<T>) -> Box<T>,
    lock: RwLock<Arc<T>>,
}

pub struct SharedImplPoisonable<T: ?Sized, C: ?Sized + SharedLockOps<T>> {
    policy: PoisonPolicy,
    poison: Poison,
    _unused: PhantomData<T>,
    container: C,
}

impl<T: ?Sized, C: ?Sized + SharedLockOps<T>> SharedLockOps<T> for SharedImplPoisonable<T, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug,
    {
        f.write_fmt(format_args!("{:?} {:?}", self.policy, self.poison))?;
        self.container.fmt(f)
    }

    fn lock_read(&self) -> SharedReadLockInner<T> {
        self.container.lock_read()
    }

    fn try_lock_read(&self) -> Option<SharedReadLockInner<T>> {
        self.container.try_lock_read()
    }

    fn lock_write(&self) -> SharedWriteLockInner<T> {
        self.container.lock_write()
    }

    fn try_lock_write(&self) -> Option<SharedWriteLockInner<T>> {
        self.container.try_lock_write()
    }
}

impl<T: ?Sized, C: ?Sized + SharedLockOps<T>> SharedImplOps<T> for SharedImplPoisonable<T, C> {
    fn get_poison(&self) -> Option<(PoisonPolicy, &Poison)> {
        Some((self.policy, &self.poison))
    }

    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        unimplemented!()
    }
}

impl<T: ?Sized> SharedLockOps<T> for RwLock<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug,
    {
        Debug::fmt(self, f)
    }

    fn lock_read(&self) -> SharedReadLockInner<T> {
        SharedReadLockInner::RwLock(self.read())
    }

    fn try_lock_read(&self) -> Option<SharedReadLockInner<T>> {
        self.try_read().map(SharedReadLockInner::RwLock)
    }

    fn lock_write(&self) -> SharedWriteLockInner<T> {
        SharedWriteLockInner::RwLock(self.write())
    }

    fn try_lock_write(&self) -> Option<SharedWriteLockInner<T>> {
        self.try_write().map(SharedWriteLockInner::RwLock)
    }
}

impl<T: ?Sized> SharedLockOps<T> for RwLock<Box<T>> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug,
    {
        Debug::fmt(self, f)
    }

    fn lock_read(&self) -> SharedReadLockInner<T> {
        SharedReadLockInner::RwLockBox(self.read())
    }

    fn try_lock_read(&self) -> Option<SharedReadLockInner<T>> {
        self.try_read().map(SharedReadLockInner::RwLockBox)
    }

    fn lock_write(&self) -> SharedWriteLockInner<T> {
        SharedWriteLockInner::RwLockBox(self.write())
    }

    fn try_lock_write(&self) -> Option<SharedWriteLockInner<T>> {
        self.try_write().map(SharedWriteLockInner::RwLockBox)
    }
}

impl<T: ?Sized> SharedLockOps<T> for Mutex<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug,
    {
        Debug::fmt(self, f)
    }

    fn lock_read(&self) -> SharedReadLockInner<T> {
        SharedReadLockInner::Mutex(self.lock())
    }

    fn try_lock_read(&self) -> Option<SharedReadLockInner<T>> {
        self.try_lock().map(SharedReadLockInner::Mutex)
    }

    fn lock_write(&self) -> SharedWriteLockInner<T> {
        SharedWriteLockInner::Mutex(self.lock())
    }

    fn try_lock_write(&self) -> Option<SharedWriteLockInner<T>> {
        self.try_lock().map(SharedWriteLockInner::Mutex)
    }
}

impl<T: ?Sized> SharedLockOps<T> for SharedImplRCU<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug,
    {
        Debug::fmt(&self.lock, f)
    }

    fn lock_read(&self) -> SharedReadLockInner<T> {
        SharedReadLockInner::ReadCopyUpdate(self.lock.read().clone())
    }

    fn try_lock_read(&self) -> Option<SharedReadLockInner<T>> {
        Some(self.lock_read())
    }

    fn lock_write(&self) -> SharedWriteLockInner<T> {
        SharedWriteLockInner::ReadCopyUpdate(&self.lock, Some((self.cloner)(&*self.lock.read())))
    }

    fn try_lock_write(&self) -> Option<SharedWriteLockInner<T>> {
        Some(self.lock_write())
    }
}

impl<T: ?Sized> SharedImplOps<T> for SharedImplRCU<T> {
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        unimplemented!()
    }
}

impl<T: ?Sized> SharedLockOps<T> for SharedImplArc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug,
    {
        Debug::fmt(&self.value, f)
    }

    fn lock_read(&self) -> SharedReadLockInner<T> {
        SharedReadLockInner::ArcRef(&self.value)
    }

    fn try_lock_read(&self) -> Option<SharedReadLockInner<T>> {
        Some(self.lock_read())
    }

    fn lock_write(&self) -> SharedWriteLockInner<T> {
        unreachable!()
    }

    fn try_lock_write(&self) -> Option<SharedWriteLockInner<T>> {
        unreachable!()
    }
}

impl<T: ?Sized> SharedImplOps<T> for SharedImplArc<T> {
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        unimplemented!()
    }
}

impl<T: ?Sized> SharedLockOps<T> for SharedImplArcBox<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug,
    {
        Debug::fmt(&self.value, f)
    }

    fn lock_read(&self) -> SharedReadLockInner<T> {
        SharedReadLockInner::ArcRef(&self.value)
    }

    fn try_lock_read(&self) -> Option<SharedReadLockInner<T>> {
        Some(self.lock_read())
    }

    fn lock_write(&self) -> SharedWriteLockInner<T> {
        unreachable!()
    }

    fn try_lock_write(&self) -> Option<SharedWriteLockInner<T>> {
        unreachable!()
    }
}

impl<T: ?Sized> SharedImplOps<T> for SharedImplArcBox<T> {
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        unimplemented!()
    }
}

impl<T: ?Sized + Debug> Debug for SharedImpl<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SharedImpl::Arc(lock) => lock.fmt(f),
            SharedImpl::ArcBox(lock) => lock.fmt(f),
            SharedImpl::ReadCopyUpdate(lock) => lock.fmt(f),
            SharedImpl::Mutex(lock) => lock.fmt(f),
            SharedImpl::RwLock(lock) => lock.fmt(f),
            SharedImpl::RwLockBox(lock) => lock.fmt(f),
            // TODO: We should format the underlying projection
            SharedImpl::Projection(_x) => f.write_fmt(format_args!("(projection)")),
            SharedImpl::ProjectionRO(_x) => f.write_fmt(format_args!("(projection)")),
        }
    }
}

impl<T: ?Sized> Clone for SharedImpl<T> {
    fn clone(&self) -> Self {
        match &self {
            SharedImpl::Arc(lock) => SharedImpl::Arc(lock.clone()),
            SharedImpl::ArcBox(lock) => SharedImpl::ArcBox(lock.clone()),
            SharedImpl::ReadCopyUpdate(lock) => SharedImpl::ReadCopyUpdate(lock.clone()),
            SharedImpl::Mutex(lock) => SharedImpl::Mutex(lock.clone()),
            SharedImpl::RwLock(lock) => SharedImpl::RwLock(lock.clone()),
            SharedImpl::RwLockBox(lock) => SharedImpl::RwLockBox(lock.clone()),
            SharedImpl::Projection(x) => SharedImpl::Projection(x.clone()),
            SharedImpl::ProjectionRO(x) => SharedImpl::ProjectionRO(x.clone()),
        }
    }
}

impl<T> SharedImpl<T> {
    /// Attempt to unwrap this synchronized object if we are the only holder of its value.
    pub fn try_unwrap(self) -> Result<T, Self> {
        match self {
            SharedImpl::Arc(x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(x.value),
                Err(x) => Err(SharedImpl::Arc(x)),
            },
            SharedImpl::ArcBox(x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(*x.value),
                Err(x) => Err(SharedImpl::ArcBox(x)),
            },
            Self::ReadCopyUpdate(lock) => match Arc::try_unwrap(lock) {
                Ok(lock) => match Arc::try_unwrap(lock.lock.into_inner()) {
                    Ok(x) => Ok(x),
                    Err(arc) => Err(SharedImpl::ReadCopyUpdate(Arc::new(make_shared_rcu(
                        lock.cloner,
                        arc,
                    )))),
                },
                Err(lock) => Err(SharedImpl::ReadCopyUpdate(lock)),
            },
            SharedImpl::Mutex(lock) => match Arc::try_unwrap(lock) {
                Ok(lock) => Ok(lock.policy.check(&lock.poison, lock.container.into_inner())),
                Err(lock) => Err(SharedImpl::Mutex(lock)),
            },
            SharedImpl::RwLock(lock) => match Arc::try_unwrap(lock) {
                Ok(lock) => Ok(lock.policy.check(&lock.poison, lock.container.into_inner())),
                Err(lock) => Err(SharedImpl::RwLock(lock)),
            },
            SharedImpl::RwLockBox(lock) => match Arc::try_unwrap(lock) {
                Ok(lock) => Ok(lock
                    .policy
                    .check(&lock.poison, *lock.container.into_inner())),
                Err(lock) => Err(SharedImpl::RwLockBox(lock)),
            },
            SharedImpl::Projection(_) => Err(self),
            SharedImpl::ProjectionRO(_) => Err(self),
        }
    }
}

impl<T: ?Sized> SharedImpl<T> {
    pub fn lock_read(&self) -> SharedReadLock<T> {
        match &self {
            SharedImpl::Arc(lock) => lock.lock_read_impl(),
            SharedImpl::ArcBox(lock) => lock.lock_read_impl(),
            SharedImpl::ReadCopyUpdate(lock) => lock.lock_read_impl(),
            SharedImpl::RwLock(lock) => lock.lock_read_impl(),
            SharedImpl::RwLockBox(lock) => lock.lock_read_impl(),
            SharedImpl::Mutex(lock) => lock.lock_read_impl(),
            SharedImpl::Projection(p) => p.lock_read(),
            SharedImpl::ProjectionRO(p) => p.read(),
        }
    }

    pub fn try_lock_read(&self) -> Option<SharedReadLock<T>> {
        match &self {
            SharedImpl::Arc(lock) => lock.try_lock_read_impl(),
            SharedImpl::ArcBox(lock) => lock.try_lock_read_impl(),
            SharedImpl::ReadCopyUpdate(lock) => lock.try_lock_read_impl(),
            SharedImpl::RwLock(lock) => lock.try_lock_read_impl(),
            SharedImpl::RwLockBox(lock) => lock.try_lock_read_impl(),
            SharedImpl::Mutex(lock) => lock.try_lock_read_impl(),
            SharedImpl::Projection(p) => p.try_lock_read(),
            SharedImpl::ProjectionRO(p) => p.try_lock_read(),
        }
    }

    pub fn lock_write(&self) -> SharedWriteLock<T> {
        match &self {
            SharedImpl::Arc(lock) => lock.lock_write_impl(),
            SharedImpl::ArcBox(lock) => lock.lock_write_impl(),
            SharedImpl::ReadCopyUpdate(lock) => lock.lock_write_impl(),
            SharedImpl::RwLock(lock) => lock.lock_write_impl(),
            SharedImpl::RwLockBox(lock) => lock.lock_write_impl(),
            SharedImpl::Mutex(lock) => lock.lock_write_impl(),
            SharedImpl::Projection(p) => p.lock_write(),
            SharedImpl::ProjectionRO(_) => unreachable!("This should not be possible"),
        }
    }

    pub fn try_lock_write(&self) -> Option<SharedWriteLock<T>> {
        match &self {
            SharedImpl::Arc(lock) => lock.try_lock_write_impl(),
            SharedImpl::ArcBox(lock) => lock.try_lock_write_impl(),
            SharedImpl::ReadCopyUpdate(lock) => lock.try_lock_write_impl(),
            SharedImpl::RwLock(lock) => lock.try_lock_write_impl(),
            SharedImpl::RwLockBox(lock) => lock.try_lock_write_impl(),
            SharedImpl::Mutex(lock) => lock.try_lock_write_impl(),
            SharedImpl::Projection(p) => p.try_lock_write(),
            SharedImpl::ProjectionRO(_) => unreachable!("This should not be possible"),
        }
    }
}

pub enum SharedGlobalImpl<T: Send> {
    Raw(T),
    RawLazy(OnceCell<T>, fn() -> T),
    RwLock(PoisonPolicy, Poison, RwLock<T>),
    RwLockLazy(PoisonPolicy, Poison, OnceCell<RwLock<T>>, fn() -> T),
    Mutex(PoisonPolicy, Poison, Mutex<T>),
    MutexLazy(PoisonPolicy, Poison, OnceCell<Mutex<T>>, fn() -> T),
}

// UNSAFETY: We cannot construct a SharedGlobalImpl that isn't safe to send across thread boundards, so we force this be to Send + Sync
unsafe impl<T: Send> Send for SharedGlobalImpl<T> {}
unsafe impl<T: Send> Sync for SharedGlobalImpl<T> {}

impl<T: Send> SharedGlobalImpl<T> {}

impl<T: Send> SharedGlobalImpl<T> {
    pub(crate) fn read(&self) -> SharedReadLock<T> {
        match self {
            SharedGlobalImpl::Raw(x) => SharedReadLock {
                inner: SharedReadLockInner::ArcRef(x),
                poison: None,
            },
            SharedGlobalImpl::RawLazy(once, f) => SharedReadLock {
                inner: SharedReadLockInner::ArcRef(once.get_or_init(f)),
                poison: None,
            },
            SharedGlobalImpl::RwLock(policy, poison, lock) => SharedReadLock {
                inner: SharedReadLockInner::RwLock(policy.check(poison, lock.read())),
                poison: policy.get_poison(poison),
            },
            SharedGlobalImpl::RwLockLazy(policy, poison, once, f) => SharedReadLock {
                inner: SharedReadLockInner::RwLock(
                    policy.check(poison, once.get_or_init(|| RwLock::new(f())).read()),
                ),
                poison: policy.get_poison(poison),
            },
            SharedGlobalImpl::Mutex(policy, poison, lock) => SharedReadLock {
                inner: SharedReadLockInner::Mutex(policy.check(poison, lock.lock())),
                poison: policy.get_poison(poison),
            },
            SharedGlobalImpl::MutexLazy(policy, poison, once, f) => SharedReadLock {
                inner: SharedReadLockInner::Mutex(
                    policy.check(poison, once.get_or_init(|| Mutex::new(f())).lock()),
                ),
                poison: policy.get_poison(poison),
            },
        }
    }

    pub(crate) fn write(&self) -> SharedWriteLock<T> {
        match self {
            SharedGlobalImpl::Raw(_) => unreachable!("Raw objects are never writable"),
            SharedGlobalImpl::RawLazy(..) => unreachable!("Raw objects are never writable"),
            SharedGlobalImpl::RwLock(policy, poison, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLock(policy.check(poison, lock.write())),
                poison: policy.get_poison(poison),
            },
            SharedGlobalImpl::RwLockLazy(policy, poison, once, f) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLock(
                    policy.check(poison, once.get_or_init(|| RwLock::new(f())).write()),
                ),
                poison: policy.get_poison(poison),
            },
            SharedGlobalImpl::Mutex(policy, poison, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::Mutex(policy.check(poison, lock.lock())),
                poison: policy.get_poison(poison),
            },
            SharedGlobalImpl::MutexLazy(policy, poison, once, f) => SharedWriteLock {
                inner: SharedWriteLockInner::Mutex(
                    policy.check(poison, once.get_or_init(|| Mutex::new(f())).lock()),
                ),
                poison: policy.get_poison(poison),
            },
        }
    }
}

impl<T: Send> SharedProjection<T> for &SharedGlobalImpl<T> {
    fn read(&self) -> SharedReadLock<T> {
        (*self).read()
    }

    fn try_lock_read(&self) -> Option<SharedReadLock<T>> {
        unimplemented!()
    }
}

impl<T: Send> SharedMutProjection<T> for &SharedGlobalImpl<T> {
    fn lock_read(&self) -> SharedReadLock<T> {
        (*self).read()
    }

    fn lock_write(&self) -> SharedWriteLock<T> {
        (*self).write()
    }

    fn try_lock_read(&self) -> Option<SharedReadLock<T>> {
        unimplemented!()
    }

    fn try_lock_write(&self) -> Option<SharedWriteLock<T>> {
        unimplemented!()
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
