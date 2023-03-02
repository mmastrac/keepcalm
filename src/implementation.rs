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

pub enum SharedMutImpl<T: ?Sized> {
    /// Only usable by non-mutable shares.
    Arc(Arc<T>),
    RwLock(PoisonPolicy, Arc<RwLock<T>>),
    /// Used for unsized types
    RwLockBox(PoisonPolicy, Arc<RwLock<Box<T>>>),
    Mutex(PoisonPolicy, Arc<Mutex<T>>),
    Projection(Arc<dyn SharedMutProjection<T> + 'static>),
    ProjectionRO(Arc<dyn SharedProjection<T> + 'static>),
}

impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for SharedMutImpl<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SharedMutImpl::Arc(x) => x.fmt(f),
            SharedMutImpl::Mutex(policy, x) => f.write_fmt(format_args!("{:?} {:?}", policy, x)),
            SharedMutImpl::RwLock(policy, x) => f.write_fmt(format_args!("{:?} {:?}", policy, x)),
            SharedMutImpl::RwLockBox(policy, x) => f.write_fmt(format_args!("{:?} {:?}", policy, x)),
            // TODO: We should format the underlying projection
            SharedMutImpl::Projection(_x) => f.write_fmt(format_args!("(projection)")),
            SharedMutImpl::ProjectionRO(_x) => f.write_fmt(format_args!("(projection)")),
        }
    }
}

impl<T: ?Sized> Clone for SharedMutImpl<T> {
    fn clone(&self) -> Self {
        match &self {
            SharedMutImpl::Arc(x) => SharedMutImpl::Arc(x.clone()),
            SharedMutImpl::RwLock(policy, x) => SharedMutImpl::RwLock(*policy, x.clone()),
            SharedMutImpl::RwLockBox(policy, x) => SharedMutImpl::RwLockBox(*policy, x.clone()),
            SharedMutImpl::Mutex(policy, x) => SharedMutImpl::Mutex(*policy, x.clone()),
            SharedMutImpl::Projection(x) => SharedMutImpl::Projection(x.clone()),
            SharedMutImpl::ProjectionRO(x) => SharedMutImpl::ProjectionRO(x.clone()),
        }
    }
}

impl<T> SharedMutImpl<T> {
    /// Attempt to unwrap this synchronized object if we are the only holder of its value.
    pub fn try_unwrap(self) -> Result<T, Self> {
        match self {
            SharedMutImpl::Arc(x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(x),
                Err(x) => Err(SharedMutImpl::Arc(x)),
            },
            SharedMutImpl::Mutex(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(policy.handle_lock(x.into_inner())),
                Err(x) => Err(SharedMutImpl::Mutex(policy, x)),
            },
            SharedMutImpl::RwLock(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(policy.handle_lock(x.into_inner())),
                Err(x) => Err(SharedMutImpl::RwLock(policy, x)),
            },
            SharedMutImpl::RwLockBox(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(*policy.handle_lock(x.into_inner())),
                Err(x) => Err(SharedMutImpl::RwLockBox(policy, x)),
            },
            SharedMutImpl::Projection(_) => Err(self),
            SharedMutImpl::ProjectionRO(_) => Err(self),
        }
    }
}

impl<T: ?Sized> SharedMutImpl<T> {
    pub fn lock_read(&self) -> SharedReadLock<T> {
        match &self {
            SharedMutImpl::Arc(x) => SharedReadLock {
                inner: SharedReadLockInner::Arc(&x),
            },
            SharedMutImpl::RwLock(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::RwLock(policy.handle_lock(lock.read())),
            },
            SharedMutImpl::RwLockBox(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::RwLockBox(policy.handle_lock(lock.read())),
            },
            SharedMutImpl::Mutex(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::Mutex(policy.handle_lock(lock.lock())),
            },
            SharedMutImpl::Projection(p) => p.lock_read(),
            SharedMutImpl::ProjectionRO(p) => p.read(),
        }
    }

    pub fn lock_write(&self) -> SharedWriteLock<T> {
        match &self {
            SharedMutImpl::Arc(_) => unreachable!("This should not be possible"),
            SharedMutImpl::RwLock(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLock(policy.handle_lock(lock.write())),
            },
            SharedMutImpl::RwLockBox(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLockBox(policy.handle_lock(lock.write())),
            },
            SharedMutImpl::Mutex(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::Mutex(policy.handle_lock(lock.lock())),
            },
            SharedMutImpl::Projection(p) => p.lock_write(),
            SharedMutImpl::ProjectionRO(_) => unreachable!("This should not be possible"),
        }
    }
}

pub trait SharedMutProjection<T: ?Sized>: Send + Sync {
    fn lock_read(&self) -> SharedReadLock<T>;
    fn lock_write(&self) -> SharedWriteLock<T>;
}

pub trait SharedProjection<T: ?Sized>: Send + Sync {
    fn read(&self) -> SharedReadLock<T>;
}
