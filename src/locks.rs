use std::sync::{MutexGuard, RwLockReadGuard, RwLockWriteGuard};

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

pub struct SharedReadLock<'a, T: ?Sized> {
    pub(crate) inner: SharedReadLockInner<'a, T>,
}

pub enum SharedWriteLockInner<'a, T: ?Sized> {
    RwLock(RwLockWriteGuard<'a, T>),
    RwLockBox(RwLockWriteGuard<'a, Box<T>>),
    Mutex(MutexGuard<'a, T>),
    Projection(Box<dyn std::ops::DerefMut<Target = T> + 'a>),
}

pub struct SharedWriteLock<'a, T: ?Sized> {
    pub(crate) inner: SharedWriteLockInner<'a, T>,
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
