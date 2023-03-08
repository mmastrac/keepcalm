use crate::{
    locks::*,
    synchronizer::{SynchronizerUnsized, SynchronizerMetadata},
};
use once_cell::sync::OnceCell;
use parking_lot::{Mutex, RwLock};
use std::{
    fmt::Debug,
    sync::{atomic::AtomicBool, Arc},
};

/// Specifies the underlying mutable synchronization primitive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ImplementationMut {
    RwLock,
    Mutex,
    Rcu,
}

/// Determines what should happen if the underlying synchronization primitive is poisoned by being held during
/// a `panic!`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PoisonPolicy {
    /// Ignore poisoned values.
    Ignore,
    /// Panic if the value is poisoned.
    Panic,
}

impl PoisonPolicy {
    fn check<T>(&self, poison: &AtomicBool, res: T) -> T {
        if *self == PoisonPolicy::Panic && poison.load(std::sync::atomic::Ordering::Acquire) {
            panic!("This lock was poisoned by a panic elsewhere in the code.");
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

pub struct LockMetadata {
    pub poison: Option<Poison>,
}

impl LockMetadata {
    pub fn poison_panic() -> Self {
        Self::poison_policy(PoisonPolicy::Panic)
    }

    pub fn poison_ignore() -> Self {
        Self::poison_policy(PoisonPolicy::Ignore)
    }

    pub fn poison_policy(policy: PoisonPolicy) -> Self {
        match policy {
            PoisonPolicy::Ignore => Self { poison: None },
            PoisonPolicy::Panic => Self {
                poison: Some(Poison::new(false)),
            },
        }
    }
}

#[allow(clippy::type_complexity)]
pub enum SharedImpl<T: ?Sized> {
    Value(SynchronizerUnsized<LockMetadata, T>),
    Box(SynchronizerUnsized<LockMetadata, Box<T>>),
    // Lazy(Arc<(OnceCell<Synchronizer<LockMetadata, T>>, fn() -> T)>),
    Projection(Arc<dyn SharedMutProjection<T> + 'static>),
    ProjectionRO(Arc<dyn SharedProjection<T> + 'static>),
}

impl<T: ?Sized + Debug> Debug for SharedImpl<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SharedImpl::Value(lock) => lock.fmt(f),
            SharedImpl::Box(lock) => lock.fmt(f),
            // TODO: We should format the underlying projection
            SharedImpl::Projection(_x) => f.write_fmt(format_args!("(projection)")),
            SharedImpl::ProjectionRO(_x) => f.write_fmt(format_args!("(projection)")),
        }
    }
}

impl<T: ?Sized> Clone for SharedImpl<T> {
    fn clone(&self) -> Self {
        match &self {
            SharedImpl::Value(lock) => SharedImpl::Value(lock.clone()),
            SharedImpl::Box(lock) => SharedImpl::Box(lock.clone()),
            SharedImpl::Projection(x) => SharedImpl::Projection(x.clone()),
            SharedImpl::ProjectionRO(x) => SharedImpl::ProjectionRO(x.clone()),
        }
    }
}

impl<T> SharedImpl<T> {
    /// Attempt to unwrap this synchronized object if we are the only holder of its value.
    pub fn try_unwrap(self) -> Result<T, Self> {
        self.check_poison();
        match self {
            SharedImpl::Value(lock) => lock.try_unwrap().map_err(SharedImpl::Value),
            SharedImpl::Box(lock) => lock.try_unwrap().map_err(SharedImpl::Box).map(|x| *x),
            SharedImpl::Projection(_) => Err(self),
            SharedImpl::ProjectionRO(_) => Err(self),
        }
    }
}

impl<T: ?Sized> SharedImpl<T> {
    fn check_poison(&self) {
        let metadata = match &self {
            SharedImpl::Value(lock) => Some(lock.metadata()),
            SharedImpl::Box(lock) => Some(lock.metadata()),
            _ => None,
        };
        if let Some(metadata) = metadata {
            if let Some(poison) = &metadata.poison {
                if poison.load(std::sync::atomic::Ordering::Acquire) {
                    panic!("This lock was poisoned by a panic elsewhere in the code.");
                }
            }
        }
    }

    pub fn lock_read(&self) -> SharedReadLock<T> {
        self.check_poison();
        match &self {
            SharedImpl::Value(lock) => lock.lock_read().into(),
            SharedImpl::Box(lock) => lock.lock_read().into(),
            SharedImpl::Projection(p) => p.lock_read(),
            SharedImpl::ProjectionRO(p) => p.read(),
        }
    }

    pub fn try_lock_read(&self) -> Option<SharedReadLock<T>> {
        self.check_poison();
        match &self {
            SharedImpl::Value(lock) => lock.try_lock_read().map(|x| x.into()),
            SharedImpl::Box(lock) => lock.try_lock_read().map(|x| x.into()),
            SharedImpl::Projection(p) => p.try_lock_read(),
            SharedImpl::ProjectionRO(p) => p.try_lock_read(),
        }
    }

    pub fn lock_write(&self) -> SharedWriteLock<T> {
        self.check_poison();
        match &self {
            SharedImpl::Value(lock) => lock.lock_write().into(),
            SharedImpl::Box(lock) => lock.lock_write().into(),
            SharedImpl::Projection(p) => p.lock_write(),
            SharedImpl::ProjectionRO(_) => unreachable!("This should not be possible"),
        }
    }

    pub fn try_lock_write(&self) -> Option<SharedWriteLock<T>> {
        self.check_poison();
        match &self {
            SharedImpl::Value(lock) => lock.try_lock_write().map(|x| x.into()),
            SharedImpl::Box(lock) => lock.try_lock_write().map(|x| x.into()),
            SharedImpl::Projection(p) => p.try_lock_write(),
            SharedImpl::ProjectionRO(_) => unreachable!("This should not be possible"),
        }
    }
}

pub enum SharedGlobalImpl<T: Send> {
    Raw(T),
    RawLazy(OnceCell<T>, fn() -> T),
    RwLock(PoisonPolicy, Poison, RwLock<T>),
    RwLockLazy(PoisonPolicy, Poison, OnceCell<RwLock<T>>, fn() -> T),
    Mutex(PoisonPolicy, Poison, Mutex<T>),
    MutexLazy(PoisonPolicy, Poison, OnceCell<Mutex<T>>, fn() -> T),
}

// UNSAFETY: We cannot construct a SharedGlobalImpl that isn't safe to send across thread boundards, so we force this be to Send + Sync
unsafe impl<T: Send> Send for SharedGlobalImpl<T> {}
unsafe impl<T: Send> Sync for SharedGlobalImpl<T> {}

impl<T: Send> SharedGlobalImpl<T> {}

impl<T: Send> SharedGlobalImpl<T> {
    pub(crate) fn read(&self) -> SharedReadLock<T> {
        match self {
            SharedGlobalImpl::Raw(x) => SharedReadLock {
                inner: SharedReadLockInner::ArcRef(x),
            },
            SharedGlobalImpl::RawLazy(once, f) => SharedReadLock {
                inner: SharedReadLockInner::ArcRef(once.get_or_init(f)),
            },
            SharedGlobalImpl::RwLock(policy, poison, lock) => SharedReadLock {
                inner: SharedReadLockInner::RwLock(policy.check(poison, lock.read())),
            },
            SharedGlobalImpl::RwLockLazy(policy, poison, once, f) => SharedReadLock {
                inner: SharedReadLockInner::RwLock(
                    policy.check(poison, once.get_or_init(|| RwLock::new(f())).read()),
                ),
            },
            SharedGlobalImpl::Mutex(policy, poison, lock) => SharedReadLock {
                inner: SharedReadLockInner::Mutex(policy.check(poison, lock.lock())),
            },
            SharedGlobalImpl::MutexLazy(policy, poison, once, f) => SharedReadLock {
                inner: SharedReadLockInner::Mutex(
                    policy.check(poison, once.get_or_init(|| Mutex::new(f())).lock()),
                ),
            },
        }
    }

    pub(crate) fn write(&self) -> SharedWriteLock<T> {
        match self {
            SharedGlobalImpl::Raw(_) => unreachable!("Raw objects are never writable"),
            SharedGlobalImpl::RawLazy(..) => unreachable!("Raw objects are never writable"),
            SharedGlobalImpl::RwLock(policy, poison, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLock(policy.check(poison, lock.write())),
            },
            SharedGlobalImpl::RwLockLazy(policy, poison, once, f) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLock(
                    policy.check(poison, once.get_or_init(|| RwLock::new(f())).write()),
                ),
            },
            SharedGlobalImpl::Mutex(policy, poison, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::Mutex(policy.check(poison, lock.lock())),
            },
            SharedGlobalImpl::MutexLazy(policy, poison, once, f) => SharedWriteLock {
                inner: SharedWriteLockInner::Mutex(
                    policy.check(poison, once.get_or_init(|| Mutex::new(f())).lock()),
                ),
            },
        }
    }
}

impl<T: Send> SharedProjection<T> for &SharedGlobalImpl<T> {
    fn read(&self) -> SharedReadLock<T> {
        (*self).read()
    }

    fn try_lock_read(&self) -> Option<SharedReadLock<T>> {
        unimplemented!()
    }
}

impl<T: Send> SharedMutProjection<T> for &SharedGlobalImpl<T> {
    fn lock_read(&self) -> SharedReadLock<T> {
        (*self).read()
    }

    fn lock_write(&self) -> SharedWriteLock<T> {
        (*self).write()
    }

    fn try_lock_read(&self) -> Option<SharedReadLock<T>> {
        unimplemented!()
    }

    fn try_lock_write(&self) -> Option<SharedWriteLock<T>> {
        unimplemented!()
    }
}

pub trait SharedMutProjection<T: ?Sized>: Send + Sync {
    fn lock_read(&self) -> SharedReadLock<T>;
    fn try_lock_read(&self) -> Option<SharedReadLock<T>>;
    fn lock_write(&self) -> SharedWriteLock<T>;
    fn try_lock_write(&self) -> Option<SharedWriteLock<T>>;
}

pub trait SharedProjection<T: ?Sized>: Send + Sync {
    fn read(&self) -> SharedReadLock<T>;
    fn try_lock_read(&self) -> Option<SharedReadLock<T>>;
}

impl<T: ?Sized> SharedProjection<T> for dyn SharedMutProjection<T> {
    fn read(&self) -> SharedReadLock<T> {
        self.lock_read()
    }

    fn try_lock_read(&self) -> Option<SharedReadLock<T>> {
        SharedMutProjection::try_lock_read(self)
    }
}
