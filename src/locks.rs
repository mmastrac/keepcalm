use std::fmt::Debug;

use crate::{
    implementation::{LockMetadata, SharedImpl},
    synchronizer::{SynchronizerMetadata, SynchronizerReadLock, SynchronizerWriteLock},
};

/// UNSAFETY: We can implement this for all types, as T must always be Send unless it is a projection, in which case the
/// projection functions must be Send.
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
}

impl<'a, T: ?Sized> Drop for SharedReadLock<'a, T> {
    fn drop(&mut self) {
        if let SharedReadLockInner::Sync(lock) = &self.inner {
            let metadata = lock.metadata();
            if let Some(poison) = &metadata.poison {
                if std::thread::panicking() {
                    poison.store(true, std::sync::atomic::Ordering::Release);
                }
            }
        }
        if let SharedReadLockInner::SyncBox(lock) = &self.inner {
            let metadata = lock.metadata();
            if let Some(poison) = &metadata.poison {
                if std::thread::panicking() {
                    poison.store(true, std::sync::atomic::Ordering::Release);
                }
            }
        }
    }
}

impl<'a, T: ?Sized> From<SynchronizerReadLock<'a, LockMetadata, T>> for SharedReadLock<'a, T> {
    fn from(value: SynchronizerReadLock<'a, LockMetadata, T>) -> Self {
        SharedReadLock {
            inner: SharedReadLockInner::Sync(value),
        }
    }
}

impl<'a, T: ?Sized> From<SynchronizerReadLock<'a, LockMetadata, Box<T>>> for SharedReadLock<'a, T> {
    fn from(value: SynchronizerReadLock<'a, LockMetadata, Box<T>>) -> Self {
        SharedReadLock {
            inner: SharedReadLockInner::SyncBox(value),
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
}

impl<'a, T: ?Sized> From<SynchronizerWriteLock<'a, LockMetadata, T>> for SharedWriteLock<'a, T> {
    fn from(value: SynchronizerWriteLock<'a, LockMetadata, T>) -> Self {
        SharedWriteLock {
            inner: SharedWriteLockInner::Sync(value),
        }
    }
}

impl<'a, T: ?Sized> From<SynchronizerWriteLock<'a, LockMetadata, Box<T>>>
    for SharedWriteLock<'a, T>
{
    fn from(value: SynchronizerWriteLock<'a, LockMetadata, Box<T>>) -> Self {
        SharedWriteLock {
            inner: SharedWriteLockInner::SyncBox(value),
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
        if let SharedWriteLockInner::Sync(lock) = &self.inner {
            let metadata = lock.metadata();
            if let Some(poison) = &metadata.poison {
                if std::thread::panicking() {
                    poison.store(true, std::sync::atomic::Ordering::Release);
                }
            }
        }
        if let SharedWriteLockInner::SyncBox(lock) = &self.inner {
            let metadata = lock.metadata();
            if let Some(poison) = &metadata.poison {
                if std::thread::panicking() {
                    poison.store(true, std::sync::atomic::Ordering::Release);
                }
            }
        }
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

/// Simple test for `Send` on the read/write locks for non-send types. Note that locks are always `Send`, as we
/// ensure that underlying locked objects are `Send` at construction time.
///
/// ```rust
/// fn ensure_send<T: Send + ?Sized>() {}
/// use keepcalm::SharedReadLock;
/// pub type Unsync = std::marker::PhantomData<std::cell::Cell<()>>;
/// pub type Unsend = std::marker::PhantomData<std::sync::MutexGuard<'static, ()>>;
/// ensure_send::<SharedReadLock<'static, Unsend>>();
/// ```
///
/// ```rust
/// fn ensure_send<T: Send + ?Sized>() {}
/// use keepcalm::SharedWriteLock;
/// pub type Unsync = std::marker::PhantomData<std::cell::Cell<()>>;
/// pub type Unsend = std::marker::PhantomData<std::sync::MutexGuard<'static, ()>>;
/// ensure_send::<SharedWriteLock<'static, Unsend>>();
/// ```
#[cfg(doctest)]
mod send_test {}

#[cfg(test)]
mod test {
    use super::*;

    /// Test that locks are Send for Send types.
    #[allow(unused)]
    #[allow(unconditional_recursion)]
    fn ensure_locks_send<T: Send>() {
        ensure_locks_send::<SharedReadLock<'static, ()>>();
        ensure_locks_send::<SharedWriteLock<'static, ()>>();
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
