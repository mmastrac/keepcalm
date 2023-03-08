use std::{fmt::Debug, sync::Arc};

use parking_lot::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::PoisonPolicy;

type Poison = std::sync::atomic::AtomicBool;

/// UNSAFETY: We can implement this for all types, as T must always be Send unless it is a projection, in which case the
/// projection functions must be Send.
unsafe impl<'a, T> Send for SynchronizerReadLock<'a, T> {}
unsafe impl<'a, T> Send for SynchronizerWriteLock<'a, T> {}

pub enum SynchronizerType {
    Arc,
    RCU,
    RwLock,
    Mutex,
}

pub enum SynchronizerReadLock<'a, T: ?Sized> {
    /// A read "lock" that's just a plain reference.
    ArcRef(&'a T),
    /// A read "lock" that's an arc, used for RCU mode.
    ReadCopyUpdate(Arc<T>),
    /// RwLock's read lock.
    RwLock(RwLockReadGuard<'a, T>),
    /// Mutex's read lock.
    Mutex(MutexGuard<'a, T>),
}

pub enum SynchronizerWriteLock<'a, T: ?Sized> {
    ReadCopyUpdate(&'a RwLock<Arc<T>>, Option<Box<T>>),
    RwLock(RwLockWriteGuard<'a, T>),
    Mutex(MutexGuard<'a, T>),
}

/// Raw implementations.
pub enum Synchronizer<T> {
    /// Only usable by non-mutable shares.
    Arc(SynchronizerArc<T>),
    /// RCU-mode, which requires us to bring a cloning function along for the ride.
    ReadCopyUpdate(SynchronizerRCU<T>),
    /// R/W lock.
    RwLock(SynchronizerRwLock<T>),
    /// Mutex.
    Mutex(SynchronizerMutex<T>),
}

/// Virtual dispatch.
macro_rules! with_ops {
    ($self:ident, $expr:ident) => {
        match $self {
            Self::Arc(x) => x.$expr(),
            Self::ReadCopyUpdate(x) => x.$expr(),
            Self::RwLock(x) => x.$expr(),
            Self::Mutex(x) => x.$expr(),
        }
    };
}

impl<T> Synchronizer<T> {
    pub fn new(policy: PoisonPolicy, sync_type: SynchronizerType, value: T) -> Synchronizer<T> {
        match sync_type {
            SynchronizerType::Arc => Synchronizer::Arc(SynchronizerArc { value }),
            SynchronizerType::RCU => unimplemented!("RCU must be called with new_cloneable"),
            SynchronizerType::Mutex => Synchronizer::Mutex(SynchronizerMutex {
                policy,
                poison: Poison::new(false),
                container: Mutex::new(value),
            }),
            SynchronizerType::RwLock => Synchronizer::RwLock(SynchronizerRwLock {
                policy,
                poison: Poison::new(false),
                container: RwLock::new(value),
            }),
        }
    }
    pub fn new_cloneable(
        policy: PoisonPolicy,
        sync_type: SynchronizerType,
        value: T,
    ) -> Synchronizer<T>
    where
        T: Clone,
    {
        match sync_type {
            SynchronizerType::RCU => Synchronizer::ReadCopyUpdate(SynchronizerRCU {
                cloner: |x| Box::new((**x).clone()),
                lock: RwLock::new(Arc::new(value)),
            }),
            _ => Self::new(policy, sync_type, value),
        }
    }
    pub fn lock_read(&self) -> SynchronizerReadLock<T> {
        with_ops!(self, lock_read)
    }
    pub fn try_lock_read(&self) -> Option<SynchronizerReadLock<T>> {
        with_ops!(self, try_lock_read)
    }
    pub fn lock_write(&self) -> SynchronizerWriteLock<T> {
        with_ops!(self, lock_write)
    }
    pub fn try_lock_write(&self) -> Option<SynchronizerWriteLock<T>> {
        with_ops!(self, try_lock_write)
    }
}

pub trait SynchronizerOps<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug;
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized;
    fn get_poison(&self) -> Option<(PoisonPolicy, &Poison)> {
        None
    }
    fn lock_read(&self) -> SynchronizerReadLock<T>;
    fn try_lock_read(&self) -> Option<SynchronizerReadLock<T>>;
    fn lock_write(&self) -> SynchronizerWriteLock<T>;
    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<T>>;
}

pub struct SynchronizerArc<T> {
    value: T,
}

pub struct SynchronizerRCU<T> {
    cloner: fn(&Arc<T>) -> Box<T>,
    lock: RwLock<Arc<T>>,
}

pub struct SynchronizerRwLock<T> {
    policy: PoisonPolicy,
    poison: Poison,
    container: RwLock<T>,
}

pub struct SynchronizerMutex<T> {
    policy: PoisonPolicy,
    poison: Poison,
    container: Mutex<T>,
}

impl<T> SynchronizerOps<T> for SynchronizerRwLock<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug,
    {
        Debug::fmt(&self.container, f)
    }

    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        Ok(self.container.into_inner())
    }

    fn lock_read(&self) -> SynchronizerReadLock<T> {
        SynchronizerReadLock::RwLock(self.container.read())
    }

    fn try_lock_read(&self) -> Option<SynchronizerReadLock<T>> {
        self.container.try_read().map(SynchronizerReadLock::RwLock)
    }

    fn lock_write(&self) -> SynchronizerWriteLock<T> {
        SynchronizerWriteLock::RwLock(self.container.write())
    }

    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<T>> {
        self.container
            .try_write()
            .map(SynchronizerWriteLock::RwLock)
    }
}

impl<T> SynchronizerOps<T> for SynchronizerMutex<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug,
    {
        Debug::fmt(&self.container, f)
    }

    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        Ok(self.container.into_inner())
    }

    fn lock_read(&self) -> SynchronizerReadLock<T> {
        SynchronizerReadLock::Mutex(self.container.lock())
    }

    fn try_lock_read(&self) -> Option<SynchronizerReadLock<T>> {
        self.container.try_lock().map(SynchronizerReadLock::Mutex)
    }

    fn lock_write(&self) -> SynchronizerWriteLock<T> {
        SynchronizerWriteLock::Mutex(self.container.lock())
    }

    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<T>> {
        self.container.try_lock().map(SynchronizerWriteLock::Mutex)
    }
}

impl<T> SynchronizerOps<T> for SynchronizerRCU<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug,
    {
        Debug::fmt(&self.lock, f)
    }

    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        let lock = self.lock.into_inner();
        match Arc::try_unwrap(lock) {
            Ok(x) => Ok(x),
            Err(lock) => Err(SynchronizerRCU {
                cloner: self.cloner,
                lock: RwLock::new(lock),
            }),
        }
    }

    fn lock_read(&self) -> SynchronizerReadLock<T> {
        SynchronizerReadLock::ReadCopyUpdate(self.lock.read().clone())
    }

    fn try_lock_read(&self) -> Option<SynchronizerReadLock<T>> {
        Some(self.lock_read())
    }

    fn lock_write(&self) -> SynchronizerWriteLock<T> {
        SynchronizerWriteLock::ReadCopyUpdate(&self.lock, Some((self.cloner)(&*self.lock.read())))
    }

    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<T>> {
        Some(self.lock_write())
    }
}

impl<T> SynchronizerOps<T> for SynchronizerArc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug,
    {
        Debug::fmt(&self.value, f)
    }

    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        Ok(self.value)
    }

    fn lock_read(&self) -> SynchronizerReadLock<T> {
        SynchronizerReadLock::ArcRef(&self.value)
    }

    fn try_lock_read(&self) -> Option<SynchronizerReadLock<T>> {
        Some(self.lock_read())
    }

    fn lock_write(&self) -> SynchronizerWriteLock<T> {
        unreachable!()
    }

    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<T>> {
        unreachable!()
    }
}
