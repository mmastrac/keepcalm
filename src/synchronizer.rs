use std::{fmt::Debug, sync::Arc};

use parking_lot::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::PoisonPolicy;

type Poison = std::sync::atomic::AtomicBool;

/// Virtual dispatch.
macro_rules! with_ops {
    ($self:ident, $expr:ident ( $( $args:expr ),* ) ) => {
        match $self {
            Self::Arc(x) => x.$expr( $( $args ),* ),
            Self::ReadCopyUpdate(x) => x.$expr( $( $args ),* ),
            Self::RwLock(x) => x.$expr( $( $args ),* ),
            Self::Mutex(x) => x.$expr( $( $args ),* ),
        }
    };
}

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
    Arc(&'a T),
    /// A read "lock" that's an arc, used for RCU mode.
    ReadCopyUpdate(Arc<T>),
    /// RwLock's read lock.
    RwLock(RwLockReadGuard<'a, T>),
    /// Mutex's read lock.
    Mutex(MutexGuard<'a, T>),
}

impl <'a, T: ?Sized> std::ops::Deref for SynchronizerReadLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        with_ops!(self, deref())
    }
}

pub enum SynchronizerWriteLock<'a, T: ?Sized> {
    Arc(&'a mut T),
    ReadCopyUpdate(RcuWriteGuard<'a, T>),
    RwLock(RwLockWriteGuard<'a, T>),
    Mutex(MutexGuard<'a, T>),
}

impl <'a, T: ?Sized> std::ops::Deref for SynchronizerWriteLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        with_ops!(self, deref())
    }
}

impl <'a, T: ?Sized> std::ops::DerefMut for SynchronizerWriteLock<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        with_ops!(self, deref_mut())
    }
}

pub struct RcuWriteGuard<'a, T: ?Sized> {
    lock: &'a RwLock<Arc<T>>,
    writer: Option<Box<T>>,
}

impl <'a, T: ?Sized> std::ops::Deref for RcuWriteGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.writer.as_deref().expect("Cannot deref after drop")
    }
}

impl <'a, T: ?Sized> std::ops::DerefMut for RcuWriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.writer.as_deref_mut().expect("Cannot deref after drop")
    }
}

impl<'a, T: ?Sized> Drop for RcuWriteGuard<'a, T> {
    fn drop(&mut self) {
        if let Some(value) = self.writer.take() {
            *self.lock.write() = value.into();
        }
    }    
}

/// Raw implementations.
pub enum Synchronizer<T: ?Sized> {
    /// Only usable by non-mutable shares.
    Arc(Arc<SynchronizerArc<T>>),
    /// RCU-mode, which requires us to bring a cloning function along for the ride.
    ReadCopyUpdate(Arc<SynchronizerRCU<T>>),
    /// R/W lock.
    RwLock(Arc<SynchronizerRwLock<T>>),
    /// Mutex.
    Mutex(Arc<SynchronizerMutex<T>>),
}

impl<T: ?Sized> Clone for Synchronizer<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Arc(x) => Self::Arc(x.clone()),
            Self::ReadCopyUpdate(x) => Self::ReadCopyUpdate(x.clone()),
            Self::RwLock(x) => Self::RwLock(x.clone()),
            Self::Mutex(x) => Self::Mutex(x.clone()),
        }
    }
}

impl<T: ?Sized> Debug for Synchronizer<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        with_ops!(self, fmt(f))
    }
}

impl<T> Synchronizer<T> {
    pub fn new(policy: PoisonPolicy, sync_type: SynchronizerType, value: T) -> Synchronizer<T> {
        match sync_type {
            SynchronizerType::Arc => Synchronizer::Arc(Arc::new(SynchronizerArc { value })),
            SynchronizerType::RCU => unimplemented!("RCU must be called with new_cloneable"),
            SynchronizerType::Mutex => Synchronizer::Mutex(Arc::new(SynchronizerMutex {
                policy,
                poison: Poison::new(false),
                container: Mutex::new(value),
            })),
            SynchronizerType::RwLock => Synchronizer::RwLock(Arc::new(SynchronizerRwLock {
                policy,
                poison: Poison::new(false),
                container: RwLock::new(value),
            })),
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
            SynchronizerType::RCU => Synchronizer::ReadCopyUpdate(Arc::new(SynchronizerRCU {
                cloner: |x| Box::new((**x).clone()),
                lock: RwLock::new(Arc::new(value)),
            })),
            _ => Self::new(policy, sync_type, value),
        }
    }

    pub fn try_unwrap(self) -> Result<T, Self> {
        macro_rules! match_arm {
            ($x:ident, $id:ident) => {
                Arc::try_unwrap($x)
                    .map_err(Self::$id)
                    .and_then(|x| x.try_unwrap_or_sync(Self::$id))
            };
        }
        match self {
            Self::Arc(x) => match_arm!(x, Arc),
            Self::ReadCopyUpdate(x) => match_arm!(x, ReadCopyUpdate),
            Self::RwLock(x) => match_arm!(x, RwLock),
            Self::Mutex(x) => match_arm!(x, Mutex),
        }
    }
}

impl<T: ?Sized> Synchronizer<T> {
    pub fn lock_read(&self) -> SynchronizerReadLock<T> {
        with_ops!(self, lock_read())
    }
    pub fn try_lock_read(&self) -> Option<SynchronizerReadLock<T>> {
        with_ops!(self, try_lock_read())
    }
    pub fn lock_write(&self) -> SynchronizerWriteLock<T> {
        with_ops!(self, lock_write())
    }
    pub fn try_lock_write(&self) -> Option<SynchronizerWriteLock<T>> {
        with_ops!(self, try_lock_write())
    }
}

pub trait SynchronizerOps<T: ?Sized> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result
    where
        T: Debug;
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized;
    fn try_unwrap_or_sync(
        self,
        f: impl Fn(Arc<Self>) -> Synchronizer<T>,
    ) -> Result<T, Synchronizer<T>>
    where
        Self: Sized,
        T: Sized,
    {
        self.try_unwrap().map_err(|x| f(Arc::new(x)))
    }
    fn get_poison(&self) -> Option<(PoisonPolicy, &Poison)> {
        None
    }
    fn lock_read(&self) -> SynchronizerReadLock<T>;
    fn try_lock_read(&self) -> Option<SynchronizerReadLock<T>>;
    fn lock_write(&self) -> SynchronizerWriteLock<T>;
    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<T>>;
}

pub struct SynchronizerArc<T: ?Sized> {
    value: T,
}

pub struct SynchronizerRCU<T: ?Sized> {
    cloner: fn(&Arc<T>) -> Box<T>,
    lock: RwLock<Arc<T>>,
}

pub struct SynchronizerRwLock<T: ?Sized> {
    policy: PoisonPolicy,
    poison: Poison,
    container: RwLock<T>,
}

pub struct SynchronizerMutex<T: ?Sized> {
    policy: PoisonPolicy,
    poison: Poison,
    container: Mutex<T>,
}

impl<T: ?Sized> SynchronizerOps<T> for SynchronizerRwLock<T> {
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

impl<T: ?Sized> SynchronizerOps<T> for SynchronizerMutex<T> {
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

impl<T: ?Sized> SynchronizerOps<T> for SynchronizerRCU<T> {
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
        SynchronizerWriteLock::ReadCopyUpdate(RcuWriteGuard {
            lock: &self.lock,
            writer: Some((self.cloner)(&*self.lock.read()))
        })
    }

    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<T>> {
        Some(self.lock_write())
    }
}

impl<T: ?Sized> SynchronizerOps<T> for SynchronizerArc<T> {
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
        SynchronizerReadLock::Arc(&self.value)
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
