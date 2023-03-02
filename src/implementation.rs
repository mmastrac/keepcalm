use crate::locks::*;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

/// Determines what should happen if the underlying synchronization primitive is poisoned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PoisonPolicy {
    Ignore,
    Panic,
}

impl PoisonPolicy {
    fn handle<T>(&self, error: PoisonError<T>) -> T {
        match self {
            PoisonPolicy::Ignore => error.into_inner(),
            PoisonPolicy::Panic => panic!("This shared object was poisoned"),
        }
    }

    fn handle_lock<T>(&self, res: Result<T, PoisonError<T>>) -> T {
        match res {
            Ok(lock) => lock,
            Err(err) => self.handle(err),
        }
    }
}

pub enum SharedRWImpl<T: ?Sized> {
    /// Only usable by non-mutable shares.
    Arc(Arc<T>),
    RwLock(PoisonPolicy, Arc<RwLock<T>>),
    /// Used for unsized types
    RwLockBox(PoisonPolicy, Arc<RwLock<Box<T>>>),
    Mutex(PoisonPolicy, Arc<Mutex<T>>),
    Projection(Arc<dyn SharedRWProjection<T> + 'static>),
}

impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for SharedRWImpl<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SharedRWImpl::Arc(x) => x.fmt(f),
            SharedRWImpl::Mutex(policy, x) => f.write_fmt(format_args!("{:?} {:?}", policy, x)),
            SharedRWImpl::RwLock(policy, x) => f.write_fmt(format_args!("{:?} {:?}", policy, x)),
            SharedRWImpl::RwLockBox(policy, x) => f.write_fmt(format_args!("{:?} {:?}", policy, x)),
            // TODO: We should format the underlying projection
            SharedRWImpl::Projection(_x) => f.write_fmt(format_args!("(projection)")),
        }
    }
}

impl<T: ?Sized> Clone for SharedRWImpl<T> {
    fn clone(&self) -> Self {
        match &self {
            SharedRWImpl::Arc(x) => SharedRWImpl::Arc(x.clone()),
            SharedRWImpl::RwLock(policy, x) => SharedRWImpl::RwLock(*policy, x.clone()),
            SharedRWImpl::RwLockBox(policy, x) => SharedRWImpl::RwLockBox(*policy, x.clone()),
            SharedRWImpl::Mutex(policy, x) => SharedRWImpl::Mutex(*policy, x.clone()),
            SharedRWImpl::Projection(x) => SharedRWImpl::Projection(x.clone()),
        }
    }
}

impl<T> SharedRWImpl<T> {
    /// Attempt to unwrap this synchronized object if we are the only holder of its value.
    pub fn try_unwrap(self) -> Result<T, Self> {
        match self {
            SharedRWImpl::Arc(x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(x),
                Err(x) => Err(SharedRWImpl::Arc(x)),
            },
            SharedRWImpl::Mutex(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(policy.handle_lock(x.into_inner())),
                Err(x) => Err(SharedRWImpl::Mutex(policy, x)),
            },
            SharedRWImpl::RwLock(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(policy.handle_lock(x.into_inner())),
                Err(x) => Err(SharedRWImpl::RwLock(policy, x)),
            },
            SharedRWImpl::RwLockBox(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(*policy.handle_lock(x.into_inner())),
                Err(x) => Err(SharedRWImpl::RwLockBox(policy, x)),
            },
            SharedRWImpl::Projection(_) => Err(self),
        }
    }
}

impl<T: ?Sized> SharedRWImpl<T> {
    pub fn lock_read(&self) -> SharedReadLock<T> {
        match &self {
            SharedRWImpl::Arc(x) => SharedReadLock {
                inner: SharedReadLockInner::Arc(&x),
            },
            SharedRWImpl::RwLock(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::RwLock(policy.handle_lock(lock.read())),
            },
            SharedRWImpl::RwLockBox(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::RwLockBox(policy.handle_lock(lock.read())),
            },
            SharedRWImpl::Mutex(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::Mutex(policy.handle_lock(lock.lock())),
            },
            SharedRWImpl::Projection(p) => p.lock_read(),
        }
    }

    pub fn lock_write(&self) -> SharedWriteLock<T> {
        match &self {
            SharedRWImpl::Arc(x) => unreachable!("This should not be possible"),
            SharedRWImpl::RwLock(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLock(policy.handle_lock(lock.write())),
            },
            SharedRWImpl::RwLockBox(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLockBox(policy.handle_lock(lock.write())),
            },
            SharedRWImpl::Mutex(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::Mutex(policy.handle_lock(lock.lock())),
            },
            SharedRWImpl::Projection(p) => p.lock_write(),
        }
    }
}

pub trait SharedRWProjection<T: ?Sized>: Send + Sync {
    fn lock_read(&self) -> SharedReadLock<T>;
    fn lock_write(&self) -> SharedWriteLock<T>;
}
