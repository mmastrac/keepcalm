use crate::{
    rcu::{RcuLock, RcuReadGuard, RcuWriteGuard},
    PoisonPolicy,
};
use parking_lot::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::{fmt::Debug, sync::Arc};

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
    ReadCopyUpdate(RcuReadGuard<T>),
    /// RwLock's read lock.
    RwLock(RwLockReadGuard<'a, T>),
    /// Mutex's read lock.
    Mutex(MutexGuard<'a, T>),
}

impl<'a, T: ?Sized> std::ops::Deref for SynchronizerReadLock<'a, T> {
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

impl<'a, T: ?Sized> std::ops::Deref for SynchronizerWriteLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        with_ops!(self, deref())
    }
}

impl<'a, T: ?Sized> std::ops::DerefMut for SynchronizerWriteLock<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        with_ops!(self, deref_mut())
    }
}

/// Raw implementations.
pub enum Synchronizer<T: ?Sized> {
    /// Only usable by non-mutable shares.
    Arc(Arc<SynchronizerImpl<T>>),
    /// RCU-mode, which requires us to bring a cloning function along for the ride.
    ReadCopyUpdate(Arc<SynchronizerImpl<RcuLock<T>>>),
    /// R/W lock.
    RwLock(Arc<SynchronizerImpl<RwLock<T>>>),
    /// Mutex.
    Mutex(Arc<SynchronizerImpl<Mutex<T>>>),
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
            SynchronizerType::Arc => Synchronizer::Arc(Arc::new(SynchronizerImpl { container: value })),
            SynchronizerType::RCU => unimplemented!("RCU must be called with new_cloneable"),
            SynchronizerType::Mutex => Synchronizer::Mutex(Arc::new(SynchronizerImpl {
                container: Mutex::new(value),
            })),
            SynchronizerType::RwLock => Synchronizer::RwLock(Arc::new(SynchronizerImpl {
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
            SynchronizerType::RCU => Synchronizer::ReadCopyUpdate(Arc::new(SynchronizerImpl {
                container: RcuLock::new(value),
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

pub struct SynchronizerImpl<C: ?Sized> {
    container: C,
}

impl <C: ?Sized> Debug for SynchronizerImpl<C> where C: Debug {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.container.fmt(f)
    }
}

impl<T: ?Sized> SynchronizerOps<T> for SynchronizerImpl<RwLock<T>> {
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

impl<T: ?Sized> SynchronizerOps<T> for SynchronizerImpl<Mutex<T>> {
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

impl<T: ?Sized> SynchronizerOps<T> for SynchronizerImpl<RcuLock<T>> {
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        self.container
            .try_unwrap()
            .map_err(|container| SynchronizerImpl { container })
    }

    fn lock_read(&self) -> SynchronizerReadLock<T> {
        SynchronizerReadLock::ReadCopyUpdate(self.container.read())
    }

    fn try_lock_read(&self) -> Option<SynchronizerReadLock<T>> {
        Some(self.lock_read())
    }

    fn lock_write(&self) -> SynchronizerWriteLock<T> {
        SynchronizerWriteLock::ReadCopyUpdate(self.container.write())
    }

    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<T>> {
        Some(self.lock_write())
    }
}

impl<T: ?Sized> SynchronizerOps<T> for SynchronizerImpl<T> {
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        Ok(self.container)
    }

    fn lock_read(&self) -> SynchronizerReadLock<T> {
        SynchronizerReadLock::Arc(&self.container)
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
