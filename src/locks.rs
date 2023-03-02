use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub enum SharedReadLockInner<'a, T: ?Sized> {
    RwLock(RwLockReadGuard<'a, T>),
    RwLockBox(RwLockReadGuard<'a, Box<T>>),
    Mutex(MutexGuard<'a, T>),
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
