use std::fmt::Debug;
use std::marker::PhantomData;

use crate::{
    implementation::{LockMetadata, SharedImpl},
    synchronizer::{SynchronizerMetadata, SynchronizerReadLock, SynchronizerWriteLock},
};

/// A zero-sized marker that removes `Send` from a lock guard.
///
/// The guards are deliberately `!Send`: a blocking lock held across an `.await` makes the
/// surrounding future `!Send`, so it cannot be spawned on a work-stealing executor (e.g.
/// `tokio::spawn`). This is a compile-time guard rail against the classic footgun of holding a lock
/// across a suspend point, which blocks the executor or deadlocks. (`std::sync::MutexGuard` is
/// `!Send`, so `PhantomData` of it removes `Send`; the guards are already `!Sync` via their
/// projection variant, so `Sync` is unaffected either way.)
type Unsend = PhantomData<std::sync::MutexGuard<'static, ()>>;

/// UNSAFETY: We can implement this for all types, as T must always be Send unless it is a projection, in which case the
/// projection functions must be Send. Note that the outer [`SharedReadLock`]/[`SharedWriteLock`]
/// are still `!Send` thanks to their [`Unsend`] marker; this inner `Send` is what lets the *owned*
/// guards (the async `spawn_blocking` vehicle) re-assert `Send` soundly.
unsafe impl<'a, T: ?Sized> Send for SharedReadLockInner<'a, T> {}
unsafe impl<'a, T: ?Sized> Send for SharedWriteLockInner<'a, T> {}

pub enum SharedReadLockInner<'a, T: ?Sized> {
    /// Delegate to Synchronizer.
    Sync(SynchronizerReadLock<'a, LockMetadata, T>),
    SyncBox(SynchronizerReadLock<'a, LockMetadata, Box<T>>),
    /// A projected lock.
    Projection(Box<dyn std::ops::Deref<Target = T> + 'a>),
}

/// This holds a read lock on the underlying container's object.
///
/// The particular behaviour of the lock depends on the underlying synchronization primitive.
#[must_use = "if unused the lock will immediately unlock"]
// Waiting for stable: https://github.com/rust-lang/rust/issues/83310
// #[must_not_suspend = "holding a lock across suspend \
//                       points can cause deadlocks, delays, \
//                       and cause Futures to not implement `Send`"]
pub struct SharedReadLock<'a, T: ?Sized> {
    pub(crate) inner: SharedReadLockInner<'a, T>,
    /// Held-lock registration for deadlock detection; `None` for untracked
    /// (Arc/RCU) synchronizers and for projections (the projected root guard
    /// carries the registration).
    #[cfg(feature = "deadlock_detection")]
    pub(crate) lockdep: Option<crate::lockdep::HeldToken>,
    /// Makes the guard `!Send` so it cannot be held across an `.await`. See [`Unsend`].
    pub(crate) _unsend: Unsend,
}

impl<'a, T: ?Sized> SharedReadLock<'a, T> {
    pub(crate) fn from_projection(inner: Box<dyn std::ops::Deref<Target = T> + 'a>) -> Self {
        SharedReadLock {
            inner: SharedReadLockInner::Projection(inner),
            #[cfg(feature = "deadlock_detection")]
            lockdep: None,
            _unsend: PhantomData,
        }
    }

    /// Map this read guard to a component of the locked data.
    ///
    /// The underlying lock stays held until the returned guard is dropped. The
    /// projection function may be called more than once.
    pub fn map<P: ?Sized, F>(self, project: F) -> SharedReadLock<'a, P>
    where
        F: for<'x> Fn(&'x T) -> &'x P + Send + 'a,
    {
        struct Mapped<'a, T: ?Sized, P: ?Sized, F> {
            lock: SharedReadLock<'a, T>,
            project: F,
            _marker: std::marker::PhantomData<&'a P>,
        }

        impl<'a, T: ?Sized, P: ?Sized, F> std::ops::Deref for Mapped<'a, T, P, F>
        where
            F: for<'x> Fn(&'x T) -> &'x P,
        {
            type Target = P;
            fn deref(&self) -> &P {
                (self.project)(&*self.lock)
            }
        }

        SharedReadLock::from_projection(Box::new(Mapped {
            lock: self,
            project,
            _marker: std::marker::PhantomData,
        }))
    }
}

impl<'a, T: ?Sized> Drop for SharedReadLock<'a, T> {
    fn drop(&mut self) {
        #[cfg(feature = "deadlock_detection")]
        if let Some(token) = self.lockdep.take() {
            crate::lockdep::release_held(token);
        }
        if let SharedReadLockInner::Sync(lock) = &self.inner {
            let metadata = lock.metadata();
            if let Some(poison) = &metadata.poison {
                if crate::implementation::panicking_should_poison() {
                    poison.store(true, std::sync::atomic::Ordering::Release);
                }
            }
        }
        if let SharedReadLockInner::SyncBox(lock) = &self.inner {
            let metadata = lock.metadata();
            if let Some(poison) = &metadata.poison {
                if crate::implementation::panicking_should_poison() {
                    poison.store(true, std::sync::atomic::Ordering::Release);
                }
            }
        }
    }
}

impl<'a, T: ?Sized> From<SynchronizerReadLock<'a, LockMetadata, T>> for SharedReadLock<'a, T> {
    fn from(value: SynchronizerReadLock<'a, LockMetadata, T>) -> Self {
        #[cfg(feature = "deadlock_detection")]
        let lockdep = crate::lockdep::read_guard_token(&value);
        SharedReadLock {
            inner: SharedReadLockInner::Sync(value),
            #[cfg(feature = "deadlock_detection")]
            lockdep,
            _unsend: PhantomData,
        }
    }
}

impl<'a, T: ?Sized> From<SynchronizerReadLock<'a, LockMetadata, Box<T>>> for SharedReadLock<'a, T> {
    fn from(value: SynchronizerReadLock<'a, LockMetadata, Box<T>>) -> Self {
        #[cfg(feature = "deadlock_detection")]
        let lockdep = crate::lockdep::read_guard_token(&value);
        SharedReadLock {
            inner: SharedReadLockInner::SyncBox(value),
            #[cfg(feature = "deadlock_detection")]
            lockdep,
            _unsend: PhantomData,
        }
    }
}

pub struct SharedReadLockOwned<T: ?Sized + 'static> {
    // Note the ordering of the fields - we want to drop the inner lock before the container!
    pub(crate) inner: SharedReadLock<'static, T>,
    // Container is never used, but we keep it around for safety/reference purposes
    #[allow(unused)]
    pub(crate) container: SharedImpl<T>,
}

// UNSAFETY: The borrowed `SharedReadLock` is `!Send` only because of its `Unsend` marker; its real
// contents are `Send` (see the `SharedReadLockInner` impl above). The owned guard is the internal
// vehicle for the async `spawn_blocking` path, which must move it across threads, so we re-assert
// `Send` here. This is *not* exposed to users -- they never receive an owned guard directly.
unsafe impl<T: ?Sized + 'static> Send for SharedReadLockOwned<T> {}
unsafe impl<T: ?Sized + 'static> Send for SharedWriteLockOwned<T> {}

impl<T: ?Sized + 'static> SharedReadLockOwned<T> {
    /// Unsafely reattach this to a lifetime. You must call this with a lock that is the same
    /// as the original lock!
    #[cfg(feature = "async_experimental")]
    #[allow(clippy::needless_lifetimes)]
    pub(crate) unsafe fn unsafe_reattach<'a>(
        self,
        _to: &'a SharedImpl<T>,
    ) -> SharedReadLock<'a, T> {
        std::mem::transmute(self.inner)
    }
}

pub struct SharedWriteLockOwned<T: ?Sized + 'static> {
    // Note the ordering of the fields - we want to drop the inner lock before the container!
    pub(crate) inner: SharedWriteLock<'static, T>,
    // Container is never used, but we keep it around for safety/reference purposes
    #[allow(unused)]
    pub(crate) container: SharedImpl<T>,
}

impl<T: ?Sized + 'static> SharedWriteLockOwned<T> {
    /// Unsafely reattach this to a lifetime. You must call this with a lock that is the same
    /// as the original lock!
    #[cfg(feature = "async_experimental")]
    #[allow(clippy::needless_lifetimes)]
    pub(crate) unsafe fn unsafe_reattach<'a>(
        self,
        _to: &'a SharedImpl<T>,
    ) -> SharedWriteLock<'a, T> {
        std::mem::transmute(self.inner)
    }
}

impl<T: ?Sized + 'static> std::ops::Deref for SharedReadLockOwned<T> {
    type Target = <SharedReadLock<'static, T> as std::ops::Deref>::Target;
    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

impl<T: ?Sized + 'static> std::ops::Deref for SharedWriteLockOwned<T> {
    type Target = <SharedWriteLock<'static, T> as std::ops::Deref>::Target;
    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

impl<T: ?Sized + 'static> std::ops::DerefMut for SharedWriteLockOwned<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.deref_mut()
    }
}

pub enum SharedWriteLockInner<'a, T: ?Sized> {
    /// Delegate to Synchronizer.
    Sync(SynchronizerWriteLock<'a, LockMetadata, T>),
    SyncBox(SynchronizerWriteLock<'a, LockMetadata, Box<T>>),
    Projection(Box<dyn std::ops::DerefMut<Target = T> + 'a>),
}

/// This holds a write lock on the underlying container's object.
///
/// The particular behaviour of the lock depends on the underlying synchronization primitive.
#[must_use = "if unused the lock will immediately unlock"]
// Waiting for stable: https://github.com/rust-lang/rust/issues/83310
// #[must_not_suspend = "holding a lock across suspend \
//                       points can cause deadlocks, delays, \
//                       and cause Futures to not implement `Send`"]
pub struct SharedWriteLock<'a, T: ?Sized> {
    pub(crate) inner: SharedWriteLockInner<'a, T>,
    /// Held-lock registration for deadlock detection; `None` for untracked
    /// (RCU) synchronizers and for projections (the projected root guard
    /// carries the registration).
    #[cfg(feature = "deadlock_detection")]
    pub(crate) lockdep: Option<crate::lockdep::HeldToken>,
    /// Makes the guard `!Send` so it cannot be held across an `.await`. See [`Unsend`].
    pub(crate) _unsend: Unsend,
}

impl<'a, T: ?Sized> SharedWriteLock<'a, T> {
    pub(crate) fn from_projection(inner: Box<dyn std::ops::DerefMut<Target = T> + 'a>) -> Self {
        SharedWriteLock {
            inner: SharedWriteLockInner::Projection(inner),
            #[cfg(feature = "deadlock_detection")]
            lockdep: None,
            _unsend: PhantomData,
        }
    }

    /// Map this write guard to a component of the locked data.
    ///
    /// The underlying lock stays held until the returned guard is dropped. The
    /// projection functions may be called more than once.
    pub fn map<P: ?Sized, FR, FW>(self, project: FR, project_mut: FW) -> SharedWriteLock<'a, P>
    where
        FR: for<'x> Fn(&'x T) -> &'x P + Send + 'a,
        FW: for<'x> Fn(&'x mut T) -> &'x mut P + Send + 'a,
    {
        struct Mapped<'a, T: ?Sized, P: ?Sized, FR, FW> {
            lock: SharedWriteLock<'a, T>,
            project: FR,
            project_mut: FW,
            _marker: std::marker::PhantomData<&'a P>,
        }

        impl<'a, T: ?Sized, P: ?Sized, FR, FW> std::ops::Deref for Mapped<'a, T, P, FR, FW>
        where
            FR: for<'x> Fn(&'x T) -> &'x P,
        {
            type Target = P;
            fn deref(&self) -> &P {
                (self.project)(&*self.lock)
            }
        }

        impl<'a, T: ?Sized, P: ?Sized, FR, FW> std::ops::DerefMut for Mapped<'a, T, P, FR, FW>
        where
            FR: for<'x> Fn(&'x T) -> &'x P,
            FW: for<'x> Fn(&'x mut T) -> &'x mut P,
        {
            fn deref_mut(&mut self) -> &mut P {
                (self.project_mut)(&mut *self.lock)
            }
        }

        SharedWriteLock::from_projection(Box::new(Mapped {
            lock: self,
            project,
            project_mut,
            _marker: std::marker::PhantomData,
        }))
    }
}

impl<'a, T: ?Sized> From<SynchronizerWriteLock<'a, LockMetadata, T>> for SharedWriteLock<'a, T> {
    fn from(value: SynchronizerWriteLock<'a, LockMetadata, T>) -> Self {
        #[cfg(feature = "deadlock_detection")]
        let lockdep = crate::lockdep::write_guard_token(&value);
        SharedWriteLock {
            inner: SharedWriteLockInner::Sync(value),
            #[cfg(feature = "deadlock_detection")]
            lockdep,
            _unsend: PhantomData,
        }
    }
}

impl<'a, T: ?Sized> From<SynchronizerWriteLock<'a, LockMetadata, Box<T>>>
    for SharedWriteLock<'a, T>
{
    fn from(value: SynchronizerWriteLock<'a, LockMetadata, Box<T>>) -> Self {
        #[cfg(feature = "deadlock_detection")]
        let lockdep = crate::lockdep::write_guard_token(&value);
        SharedWriteLock {
            inner: SharedWriteLockInner::SyncBox(value),
            #[cfg(feature = "deadlock_detection")]
            lockdep,
            _unsend: PhantomData,
        }
    }
}

impl<'a, T: ?Sized> std::ops::Deref for SharedReadLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        use SharedReadLockInner::*;
        match &self.inner {
            Sync(x) => x,
            SyncBox(x) => x,
            Projection(x) => x,
        }
    }
}

impl<'a, T: ?Sized> std::ops::Deref for SharedWriteLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        use SharedWriteLockInner::*;
        match &self.inner {
            Sync(x) => x,
            SyncBox(x) => x,
            Projection(x) => x,
        }
    }
}

impl<'a, T: ?Sized> std::ops::DerefMut for SharedWriteLock<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        use SharedWriteLockInner::*;
        match &mut self.inner {
            Sync(x) => &mut *x,
            SyncBox(x) => &mut *x,
            Projection(x) => &mut *x,
        }
    }
}

impl<'a, T: ?Sized> Drop for SharedWriteLock<'a, T> {
    fn drop(&mut self) {
        #[cfg(feature = "deadlock_detection")]
        if let Some(token) = self.lockdep.take() {
            crate::lockdep::release_held(token);
        }
        if let SharedWriteLockInner::Sync(lock) = &self.inner {
            let metadata = lock.metadata();
            if let Some(poison) = &metadata.poison {
                if crate::implementation::panicking_should_poison() {
                    poison.store(true, std::sync::atomic::Ordering::Release);
                }
            }
        }
        if let SharedWriteLockInner::SyncBox(lock) = &self.inner {
            let metadata = lock.metadata();
            if let Some(poison) = &metadata.poison {
                if crate::implementation::panicking_should_poison() {
                    poison.store(true, std::sync::atomic::Ordering::Release);
                }
            }
        }
    }
}

/// A `Send` lock guard, produced by the opt-in `*_send` accessors ([`crate::SharedMut::read_send`],
/// [`crate::SharedMut::write_send`], [`crate::Shared::read_send`]).
///
/// The ordinary guards returned by `read`/`write` are deliberately `!Send`, so that a lock held
/// across an `.await` makes the surrounding future `!Send` and therefore unspawnable on a
/// work-stealing executor — a compile-time guard rail against blocking or deadlocking the executor.
/// A `SendGuard` opts *out* of that protection.
///
/// **Only reach for this if you understand the consequences.** A `Send` guard can be moved to
/// another thread and, in `async` code, held across a suspend point. Holding a *blocking* lock
/// across `.await` can stall the executor thread or deadlock it; moving a live guard between threads
/// changes which thread ultimately releases the lock. You are responsible for ensuring neither is a
/// problem. When in doubt, use `read`/`write` instead and keep the guard on one thread, released
/// before the next `.await`.
#[must_use = "if unused the lock will immediately unlock"]
pub struct SendGuard<G> {
    inner: G,
}

// UNSAFETY: The wrapped guard is `!Send` only because of its `Unsend` marker; its actual contents
// are `Send` (the underlying parking_lot guard uses the `send_guard` feature and any projection
// closures are `Send + Sync` by construction). `SendGuard::new` is `pub(crate)` and only ever wraps
// our own guards, so re-asserting `Send` here is sound. The user opts in explicitly via `*_send`.
unsafe impl<G> Send for SendGuard<G> {}

impl<G> SendGuard<G> {
    pub(crate) fn new(inner: G) -> Self {
        SendGuard { inner }
    }

    /// Unwrap back into the underlying (`!Send`) guard.
    pub fn into_inner(self) -> G {
        self.inner
    }
}

impl<G: std::ops::Deref> std::ops::Deref for SendGuard<G> {
    type Target = G::Target;
    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

impl<G: std::ops::DerefMut> std::ops::DerefMut for SendGuard<G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.deref_mut()
    }
}

impl<G: Debug> Debug for SendGuard<G> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

impl<G: std::fmt::Display> std::fmt::Display for SendGuard<G> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner.fmt(f)
    }
}

/// Defines some common delegated operations on the underlying values, allowing the consumer to avoid having to dereference the
/// lock directly. This could likely be made generic rather than using macros.
macro_rules! implement_lock_delegates {
    ($for:ty) => {
        /// Implement Debug where T: Debug.
        impl<'a, T: ?Sized> std::fmt::Debug for $for
        where
            T: Debug,
        {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                (**self).fmt(f)
            }
        }

        /// Implement Display where T: Display.
        impl<'a, T: ?Sized> std::fmt::Display for $for
        where
            T: std::fmt::Display,
        {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                (**self).fmt(f)
            }
        }

        /// Implement Error where T: Error
        impl<'a, T: ?Sized> std::error::Error for $for
        where
            T: std::error::Error,
        {
            #[allow(deprecated)]
            fn cause(&self) -> Option<&dyn std::error::Error> {
                (**self).cause()
            }

            #[allow(deprecated)]
            fn description(&self) -> &str {
                (**self).description()
            }

            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                (**self).source()
            }
        }

        /// Implement Borrow
        impl<'a, T: ?Sized> std::borrow::Borrow<T> for $for {
            fn borrow(&self) -> &T {
                &**self
            }
        }

        /// Implement AsRef where T: AsRef
        impl<'a, T: ?Sized, U: ?Sized> AsRef<U> for $for
        where
            T: AsRef<U>,
        {
            fn as_ref(&self) -> &U {
                (**self).as_ref()
            }
        }

        /// Implement AsMut where T: AsMut
        impl<'a, T: ?Sized, U: ?Sized> AsMut<U> for $for
        where
            T: AsMut<U>,
            Self: std::ops::DerefMut<Target = T>,
        {
            fn as_mut(&mut self) -> &mut U {
                (**self).as_mut()
            }
        }

        /// Implement PartialEq, but only for raw types.
        impl<'a, T: ?Sized, Rhs: ?Sized> PartialEq<Rhs> for $for
        where
            T: PartialEq<Rhs>,
        {
            fn eq(&self, other: &Rhs) -> bool {
                (**self).eq(other)
            }
        }

        /// Implement PartialOrd, but only for raw types.
        impl<'a, T: ?Sized, Rhs: ?Sized> PartialOrd<Rhs> for $for
        where
            T: PartialOrd<Rhs>,
        {
            fn partial_cmp(&self, other: &Rhs) -> Option<std::cmp::Ordering> {
                (**self).partial_cmp(other)
            }
        }

        impl<'a, T: ?Sized> Unpin for $for {}
    };
}

implement_lock_delegates!(SharedReadLock<'a, T>);
implement_lock_delegates!(SharedReadLockOwned<T>);
implement_lock_delegates!(SharedWriteLock<'a, T>);
implement_lock_delegates!(SharedWriteLockOwned<T>);

/// The read/write guards are deliberately `!Send` so they cannot be held across an `.await`. These
/// assertions must fail to compile:
///
/// ```compile_fail
/// fn ensure_send<T: Send + ?Sized>() {}
/// use keepcalm::SharedReadLock;
/// ensure_send::<SharedReadLock<'static, i32>>();
/// ```
///
/// ```compile_fail
/// fn ensure_send<T: Send + ?Sized>() {}
/// use keepcalm::SharedWriteLock;
/// ensure_send::<SharedWriteLock<'static, i32>>();
/// ```
///
/// A future that holds a guard across an `.await` is therefore `!Send` and cannot be spawned:
///
/// ```compile_fail
/// use keepcalm::SharedMut;
/// fn ensure_send<T: Send>(t: T) -> T { t }
/// let shared = SharedMut::new(1);
/// let fut = async move {
///     let guard = shared.write();
///     std::future::ready(()).await; // holding the guard across a suspend point
///     *guard += 1;
/// };
/// ensure_send(fut); // fails: `fut` is `!Send` because it holds a `!Send` guard across `.await`
/// ```
///
/// The opt-in [`keepcalm::SendGuard`] (via `read_send`/`write_send`) *is* `Send`:
///
/// ```rust
/// use keepcalm::{SendGuard, SharedReadLock, SharedMut};
/// fn ensure_send<T: Send>() {}
/// ensure_send::<SendGuard<SharedReadLock<'static, i32>>>();
/// let shared = SharedMut::new(1);
/// let g = shared.read_send();
/// assert_eq!(*g, 1);
/// ```
#[cfg(doctest)]
mod send_test {}

#[cfg(test)]
mod test {
    use super::*;

    /// The borrowed guards are `!Send` (asserted via the `send_test` compile_fail doctests). The
    /// opt-in `SendGuard` wrapper re-enables `Send`.
    #[allow(unused)]
    fn ensure_guard_auto_traits() {
        fn ensure_send<T: Send>() {}
        ensure_send::<SendGuard<SharedReadLock<'static, ()>>>();
        ensure_send::<SendGuard<SharedWriteLock<'static, ()>>>();
    }

    #[test]
    fn test_guard_map() {
        let shared = crate::SharedMut::new((1u32, "x".to_string()));

        let read = shared.read().map(|t| &t.0);
        assert_eq!(*read, 1);
        // The root lock is still held by the mapped guard.
        assert!(shared.try_write().is_none());
        drop(read);

        let mut write = shared.write().map(|t| &t.1, |t| &mut t.1);
        *write += "y";
        drop(write);
        assert_eq!(shared.read().1, "xy");
    }

    #[allow(unused)]
    fn ensure_locks_coerce_deref(read: SharedReadLock<String>) {
        fn takes_as_ref(_: &String) {}
        fn takes_as_ref_str(_: &impl AsRef<str>) {}

        takes_as_ref(&read);
        takes_as_ref_str(&read);
        assert!(read == *"123");
    }
}
