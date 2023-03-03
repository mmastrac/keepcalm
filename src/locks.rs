use std::sync::atomic::AtomicBool;

use parking_lot::{MutexGuard, RwLockReadGuard, RwLockWriteGuard};

/// UNSAFETY: We can implement this iff T: Send
unsafe impl<'a, T: Send> Send for SharedReadLockInner<'a, T> {}
unsafe impl<'a, T: Send> Send for SharedWriteLockInner<'a, T> {}

pub enum SharedReadLockInner<'a, T: ?Sized> {
    /// A read "lock" that's just a plain reference.
    Arc(&'a T),
    /// RwLock's read lock.
    RwLock(RwLockReadGuard<'a, T>),
    /// RwLock's read lock, but for a Box.
    RwLockBox(RwLockReadGuard<'a, Box<T>>),
    /// Mutex's read lock.
    Mutex(MutexGuard<'a, T>),
    /// A projected lock.
    Projection(Box<dyn std::ops::Deref<Target = T> + 'a>),
}

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
    RwLock(RwLockWriteGuard<'a, T>),
    RwLockBox(RwLockWriteGuard<'a, Box<T>>),
    Mutex(MutexGuard<'a, T>),
    Projection(Box<dyn std::ops::DerefMut<Target = T> + 'a>),
}

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
            Arc(x) => x,
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
    }
}

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
}
