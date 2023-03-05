use std::{
    fmt::Debug,
    sync::{atomic::AtomicBool, Arc},
};

use parking_lot::{MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// UNSAFETY: We can implement this iff T: Send
unsafe impl<'a, T: Send> Send for SharedReadLockInner<'a, T> {}
unsafe impl<'a, T: Send> Send for SharedWriteLockInner<'a, T> {}

pub enum SharedReadLockInner<'a, T: ?Sized> {
    /// A read "lock" that's just a plain reference.
    ArcRef(&'a T),
    /// A read "lock" that's an arc, used for RCU mode.
    ReadCopyUpdate(Arc<T>),
    /// RwLock's read lock.
    RwLock(RwLockReadGuard<'a, T>),
    /// RwLock's read lock, but for a Box.
    RwLockBox(RwLockReadGuard<'a, Box<T>>),
    /// Mutex's read lock.
    Mutex(MutexGuard<'a, T>),
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
    pub(crate) poison: Option<&'a AtomicBool>,
}

impl<'a, T: ?Sized> Drop for SharedReadLock<'a, T> {
    fn drop(&mut self) {
        if let Some(poison) = self.poison {
            if std::thread::panicking() {
                poison.store(true, std::sync::atomic::Ordering::Release);
            }
        }
    }
}

pub enum SharedWriteLockInner<'a, T: ?Sized> {
    ReadCopyUpdate(&'a RwLock<Arc<T>>, Option<Box<T>>),
    RwLock(RwLockWriteGuard<'a, T>),
    RwLockBox(RwLockWriteGuard<'a, Box<T>>),
    Mutex(MutexGuard<'a, T>),
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
    pub(crate) poison: Option<&'a AtomicBool>,
}

impl<'a, T: ?Sized> std::ops::Deref for SharedReadLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        use SharedReadLockInner::*;
        match &self.inner {
            ArcRef(x) => x,
            ReadCopyUpdate(x) => x,
            RwLock(x) => x,
            RwLockBox(x) => x,
            Mutex(x) => x,
            Projection(x) => x,
        }
    }
}

impl<'a, T: ?Sized> std::ops::Deref for SharedWriteLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        use SharedWriteLockInner::*;
        match &self.inner {
            ReadCopyUpdate(_, x) => x.as_ref().expect("Cannot deref after drop"),
            RwLock(x) => x,
            RwLockBox(x) => x,
            Mutex(x) => x,
            Projection(x) => x,
        }
    }
}

impl<'a, T: ?Sized> std::ops::DerefMut for SharedWriteLock<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        use SharedWriteLockInner::*;
        match &mut self.inner {
            ReadCopyUpdate(_, x) => x.as_mut().expect("Cannot deref after drop"),
            RwLock(x) => &mut *x,
            RwLockBox(x) => &mut *x,
            Mutex(x) => &mut *x,
            Projection(x) => &mut *x,
        }
    }
}

impl<'a, T: ?Sized> Drop for SharedWriteLock<'a, T> {
    fn drop(&mut self) {
        if let Some(poison) = self.poison {
            if std::thread::panicking() {
                poison.store(true, std::sync::atomic::Ordering::Release);
            }
        }

        if let SharedWriteLockInner::ReadCopyUpdate(a, b) = &mut self.inner {
            if let Some(b) = b.take() {
                *a.write() = b.into();
            }
        }
    }
}

/// Defines some common delegated operations on the underlying values, allowing the consumer to avoid having to dereference the
/// lock directly. This could likely be made generic rather than using macros.
macro_rules! implement_lock_delegates {
    ($for:ident) => {
        /// Implement Debug where T: Debug.
        impl<'a, T: ?Sized> std::fmt::Debug for $for<'a, T>
        where
            T: Debug,
        {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                (**self).fmt(f)
            }
        }

        /// Implement Display where T: Display.
        impl<'a, T: ?Sized> std::fmt::Display for $for<'a, T>
        where
            T: std::fmt::Display,
        {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                (**self).fmt(f)
            }
        }

        /// Implement Error where T: Error
        impl<'a, T: ?Sized> std::error::Error for $for<'a, T>
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
        impl<'a, T: ?Sized> std::borrow::Borrow<T> for $for<'a, T> {
            fn borrow(&self) -> &T {
                &**self
            }
        }

        /// Implement AsRef where T: AsRef
        impl<'a, T: ?Sized, U: ?Sized> AsRef<U> for $for<'a, T>
        where
            T: AsRef<U>,
        {
            fn as_ref(&self) -> &U {
                (**self).as_ref()
            }
        }

        /// Implement AsMut where T: AsMut
        impl<'a, T: ?Sized, U: ?Sized> AsMut<U> for $for<'a, T>
        where
            T: AsMut<U>,
            Self: std::ops::DerefMut<Target = T>,
        {
            fn as_mut(&mut self) -> &mut U {
                (**self).as_mut()
            }
        }

        /// Implement PartialEq, but only for raw types.
        impl<'a, T: ?Sized, Rhs: ?Sized> PartialEq<Rhs> for $for<'a, T>
        where
            T: PartialEq<Rhs>,
        {
            fn eq(&self, other: &Rhs) -> bool {
                (**self).eq(other)
            }
        }

        /// Implement PartialOrd, but only for raw types.
        impl<'a, T: ?Sized, Rhs: ?Sized> PartialOrd<Rhs> for $for<'a, T>
        where
            T: PartialOrd<Rhs>,
        {
            fn partial_cmp(&self, other: &Rhs) -> Option<std::cmp::Ordering> {
                (**self).partial_cmp(other)
            }
        }

        impl<'a, T: ?Sized> Unpin for $for<'a, T> {}
    };
}

implement_lock_delegates!(SharedReadLock);
implement_lock_delegates!(SharedWriteLock);

/// Simple compile_fail test for Send on the read/write locks for non-send types.
///
/// ```rust compile_fail
/// fn ensure_send<T: Send + ?Sized>() {}
/// use keepcalm::SharedReadLock;
/// pub type Unsync = std::marker::PhantomData<std::cell::Cell<()>>;
/// pub type Unsend = std::marker::PhantomData<std::sync::MutexGuard<'static, ()>>;
/// ensure_send::<SharedReadLock<'static, Unsend>>();
/// ```
///
/// ```rust compile_fail
/// fn ensure_send<T: Send + ?Sized>() {}
/// use keepcalm::SharedReadLock;
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
        read == "123".to_string();
    }
}
