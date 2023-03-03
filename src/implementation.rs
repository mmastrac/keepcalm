use crate::locks::*;
use parking_lot::{Mutex, RwLock};
use std::sync::{atomic::AtomicBool, Arc};

/// Determines what should happen if the underlying synchronization primitive is poisoned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PoisonPolicy {
    Ignore,
    Panic,
}

impl PoisonPolicy {
    fn check<T>(&self, poison: &AtomicBool, res: T) -> T {
        if *self == PoisonPolicy::Panic {
            if poison.load(std::sync::atomic::Ordering::Acquire) {
                panic!("This lock was poisoned by a panic elsewhere in the code.");
            }
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

pub enum SharedImpl<T: ?Sized> {
    /// Only usable by non-mutable shares.
    Arc(Arc<T>),
    RwLock(PoisonPolicy, Arc<(Poison, RwLock<T>)>),
    /// Used for unsized types
    RwLockBox(PoisonPolicy, Arc<(Poison, RwLock<Box<T>>)>),
    Mutex(PoisonPolicy, Arc<(Poison, Mutex<T>)>),
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
                Ok(x) => Ok(policy.check(&x.0, x.1.into_inner())),
                Err(x) => Err(SharedImpl::Mutex(policy, x)),
            },
            SharedImpl::RwLock(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(policy.check(&x.0, x.1.into_inner())),
                Err(x) => Err(SharedImpl::RwLock(policy, x)),
            },
            SharedImpl::RwLockBox(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(*policy.check(&x.0, x.1.into_inner())),
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
                poison: None,
            },
            SharedImpl::RwLock(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::RwLock(policy.check(&lock.0, lock.1.read())),
                poison: policy.get_poison(&lock.0),
            },
            SharedImpl::RwLockBox(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::RwLockBox(policy.check(&lock.0, lock.1.read())),
                poison: policy.get_poison(&lock.0),
            },
            SharedImpl::Mutex(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::Mutex(policy.check(&lock.0, lock.1.lock())),
                poison: policy.get_poison(&lock.0),
            },
            SharedImpl::Projection(p) => p.lock_read(),
            SharedImpl::ProjectionRO(p) => p.read(),
        }
    }

    pub fn lock_write(&self) -> SharedWriteLock<T> {
        match &self {
            SharedImpl::Arc(_) => unreachable!("This should not be possible"),
            SharedImpl::RwLock(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLock(policy.check(&lock.0, lock.1.write())),
                poison: policy.get_poison(&lock.0),
            },
            SharedImpl::RwLockBox(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLockBox(policy.check(&lock.0, lock.1.write())),
                poison: policy.get_poison(&lock.0),
            },
            SharedImpl::Mutex(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::Mutex(policy.check(&lock.0, lock.1.lock())),
                poison: policy.get_poison(&lock.0),
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
