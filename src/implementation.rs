use crate::locks::*;
use parking_lot::{Mutex, RwLock};
use std::sync::Arc;

/// Determines what should happen if the underlying synchronization primitive is poisoned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PoisonPolicy {
    Ignore,
    Panic,
}

impl PoisonPolicy {
    fn handle_lock<T>(&self, res: T) -> T {
        res
    }
    // fn handle<T>(&self, error: PoisonError<T>) -> T {
    //     match self {
    //         PoisonPolicy::Ignore => error.into_inner(),
    //         PoisonPolicy::Panic => panic!("This shared object was poisoned"),
    //     }
    // }

    // fn handle_lock<T>(&self, res: Result<T, PoisonError<T>>) -> T {
    //     match res {
    //         Ok(lock) => lock,
    //         Err(err) => self.handle(err),
    //     }
    // }
}

pub enum SharedImpl<T: ?Sized> {
    /// Only usable by non-mutable shares.
    Arc(Arc<T>),
    RwLock(PoisonPolicy, Arc<RwLock<T>>),
    /// Used for unsized types
    RwLockBox(PoisonPolicy, Arc<RwLock<Box<T>>>),
    Mutex(PoisonPolicy, Arc<Mutex<T>>),
    Projection(Arc<dyn SharedMutProjection<T> + 'static>),
    ProjectionRO(Arc<dyn SharedProjection<T> + 'static>),
}

impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for SharedImpl<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SharedImpl::Arc(x) => x.fmt(f),
            SharedImpl::Mutex(policy, x) => f.write_fmt(format_args!("{:?} {:?}", policy, x)),
            SharedImpl::RwLock(policy, x) => f.write_fmt(format_args!("{:?} {:?}", policy, x)),
            SharedImpl::RwLockBox(policy, x) => f.write_fmt(format_args!("{:?} {:?}", policy, x)),
            // TODO: We should format the underlying projection
            SharedImpl::Projection(_x) => f.write_fmt(format_args!("(projection)")),
            SharedImpl::ProjectionRO(_x) => f.write_fmt(format_args!("(projection)")),
        }
    }
}

impl<T: ?Sized> Clone for SharedImpl<T> {
    fn clone(&self) -> Self {
        match &self {
            SharedImpl::Arc(x) => SharedImpl::Arc(x.clone()),
            SharedImpl::RwLock(policy, x) => SharedImpl::RwLock(*policy, x.clone()),
            SharedImpl::RwLockBox(policy, x) => SharedImpl::RwLockBox(*policy, x.clone()),
            SharedImpl::Mutex(policy, x) => SharedImpl::Mutex(*policy, x.clone()),
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
                Ok(x) => Ok(x),
                Err(x) => Err(SharedImpl::Arc(x)),
            },
            SharedImpl::Mutex(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(policy.handle_lock(x.into_inner())),
                Err(x) => Err(SharedImpl::Mutex(policy, x)),
            },
            SharedImpl::RwLock(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(policy.handle_lock(x.into_inner())),
                Err(x) => Err(SharedImpl::RwLock(policy, x)),
            },
            SharedImpl::RwLockBox(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(*policy.handle_lock(x.into_inner())),
                Err(x) => Err(SharedImpl::RwLockBox(policy, x)),
            },
            SharedImpl::Projection(_) => Err(self),
            SharedImpl::ProjectionRO(_) => Err(self),
        }
    }
}

impl<T: ?Sized> SharedImpl<T> {
    pub fn lock_read(&self) -> SharedReadLock<T> {
        match &self {
            SharedImpl::Arc(x) => SharedReadLock {
                inner: SharedReadLockInner::Arc(x),
            },
            SharedImpl::RwLock(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::RwLock(policy.handle_lock(lock.read())),
            },
            SharedImpl::RwLockBox(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::RwLockBox(policy.handle_lock(lock.read())),
            },
            SharedImpl::Mutex(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::Mutex(policy.handle_lock(lock.lock())),
            },
            SharedImpl::Projection(p) => p.lock_read(),
            SharedImpl::ProjectionRO(p) => p.read(),
        }
    }

    pub fn lock_write(&self) -> SharedWriteLock<T> {
        match &self {
            SharedImpl::Arc(_) => unreachable!("This should not be possible"),
            SharedImpl::RwLock(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLock(policy.handle_lock(lock.write())),
            },
            SharedImpl::RwLockBox(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLockBox(policy.handle_lock(lock.write())),
            },
            SharedImpl::Mutex(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::Mutex(policy.handle_lock(lock.lock())),
            },
            SharedImpl::Projection(p) => p.lock_write(),
            SharedImpl::ProjectionRO(_) => unreachable!("This should not be possible"),
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
