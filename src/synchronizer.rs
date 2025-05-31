use crate::rcu::{RcuLock, RcuReadGuard, RcuWriteGuard};
use parking_lot::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::{fmt::Debug, sync::Arc};

/// Virtual dispatch.
macro_rules! with_ops {
    ($self:ident: $( $pat:pat ),+ => $expr:expr ) => {
        match $self {
            Self::Arc($($pat),+) => $expr,
            Self::ReadCopyUpdate($($pat),+) => $expr,
            Self::RwLock($($pat),+) => $expr,
            Self::Mutex($($pat),+) => $expr,
        }
    };
}

/// UNSAFETY: We can implement this for all types, as T must always be Send unless it is a projection, in which case the
/// projection functions must be Send.
unsafe impl<'a, M: Send + Sync, T> Send for SynchronizerReadLock<'a, M, T> {}
unsafe impl<'a, M: Send + Sync, T> Send for SynchronizerWriteLock<'a, M, T> {}

#[derive(Clone, Copy)]
pub enum SynchronizerType {
    Arc,
    Rcu,
    RwLock,
    Mutex,
}

pub trait SynchronizerMetadata<M> {
    fn metadata(&self) -> &M;
}

pub enum SynchronizerReadLock<'a, M, T: ?Sized> {
    /// A read "lock" that's just a plain reference.
    Arc(&'a M, &'a T),
    /// A read "lock" that's an arc, used for RCU mode.
    ReadCopyUpdate(&'a M, RcuReadGuard<T>),
    /// RwLock's read lock.
    RwLock(&'a M, RwLockReadGuard<'a, T>),
    /// Mutex's read lock.
    Mutex(&'a M, MutexGuard<'a, T>),
}

impl<'a, M, T: ?Sized> std::ops::Deref for SynchronizerReadLock<'a, M, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        with_ops!(self: _, x => x)
    }
}

impl<'a, M, T: ?Sized> SynchronizerMetadata<M> for SynchronizerReadLock<'a, M, T> {
    fn metadata(&self) -> &M {
        with_ops!(self: x, _ => x)
    }
}

pub enum SynchronizerWriteLock<'a, M, T: ?Sized> {
    Arc(&'a M, &'a mut T),
    ReadCopyUpdate(&'a M, RcuWriteGuard<'a, T>),
    RwLock(&'a M, RwLockWriteGuard<'a, T>),
    Mutex(&'a M, MutexGuard<'a, T>),
}

impl<'a, M, T: ?Sized> std::ops::Deref for SynchronizerWriteLock<'a, M, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        with_ops!(self: _, x => x.deref())
    }
}

impl<'a, M, T: ?Sized> std::ops::DerefMut for SynchronizerWriteLock<'a, M, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        with_ops!(self: _, x => x.deref_mut())
    }
}

impl<'a, M, T: ?Sized> SynchronizerMetadata<M> for SynchronizerWriteLock<'a, M, T> {
    fn metadata(&self) -> &M {
        with_ops!(self: x, _ => x)
    }
}

/// Raw implementations.
pub enum SynchronizerSized<M, T> {
    /// Only usable by non-mutable shares.
    Arc(SynchronizerImpl<M, T>),
    /// RCU-mode, which requires us to bring a cloning function along for the ride.
    ReadCopyUpdate(SynchronizerImpl<M, RcuLock<T>>),
    /// R/W lock.
    RwLock(SynchronizerImpl<M, RwLock<T>>),
    /// Mutex.
    Mutex(SynchronizerImpl<M, Mutex<T>>),
}

impl<M, T> SynchronizerSized<M, T> {
    pub fn lock_read(&self) -> SynchronizerReadLock<M, T> {
        with_ops!(self: x => x.lock_read())
    }
    pub fn try_lock_read(&self) -> Option<SynchronizerReadLock<M, T>> {
        with_ops!(self: x => x.try_lock_read())
    }
    pub fn lock_write(&self) -> SynchronizerWriteLock<M, T> {
        with_ops!(self: x => x.lock_write())
    }
    pub fn try_lock_write(&self) -> Option<SynchronizerWriteLock<M, T>> {
        with_ops!(self: x => x.try_lock_write())
    }
}

impl<M, T> SynchronizerSized<M, T> {
    pub const fn new(metadata: M, sync_type: SynchronizerType, value: T) -> Self {
        match sync_type {
            SynchronizerType::Arc => Self::Arc(SynchronizerImpl {
                metadata,
                container: value,
            }),
            SynchronizerType::Rcu => panic!("RCU must be called with new_cloneable"),
            SynchronizerType::Mutex => Self::Mutex(SynchronizerImpl {
                metadata,
                container: Mutex::new(value),
            }),
            SynchronizerType::RwLock => Self::RwLock(SynchronizerImpl {
                metadata,
                container: RwLock::new(value),
            }),
        }
    }

    pub fn new_cloneable(metadata: M, sync_type: SynchronizerType, value: T) -> Self
    where
        T: Clone,
    {
        match sync_type {
            SynchronizerType::Rcu => Self::ReadCopyUpdate(SynchronizerImpl {
                metadata,
                container: RcuLock::new(value),
            }),
            _ => Self::new(metadata, sync_type, value),
        }
    }
}

/// Raw implementations. Note that because Rust doesn't support unsized enums, the [`std::sync::Arc`] lives inside
/// of the enum.
pub enum SynchronizerUnsized<M, T: ?Sized> {
    /// Only usable by non-mutable shares.
    Arc(Arc<SynchronizerImpl<M, T>>),
    /// RCU-mode, which requires us to bring a cloning function along for the ride.
    ReadCopyUpdate(Arc<SynchronizerImpl<M, RcuLock<T>>>),
    /// R/W lock.
    RwLock(Arc<SynchronizerImpl<M, RwLock<T>>>),
    /// Mutex.
    Mutex(Arc<SynchronizerImpl<M, Mutex<T>>>),
}

impl<M, T: ?Sized> SynchronizerMetadata<M> for SynchronizerUnsized<M, T> {
    fn metadata(&self) -> &M {
        with_ops!(self: x => &x.metadata)
    }
}

impl<M, T: ?Sized> Clone for SynchronizerUnsized<M, T> {
    fn clone(&self) -> Self {
        match self {
            Self::Arc(x) => Self::Arc(x.clone()),
            Self::ReadCopyUpdate(x) => Self::ReadCopyUpdate(x.clone()),
            Self::RwLock(x) => Self::RwLock(x.clone()),
            Self::Mutex(x) => Self::Mutex(x.clone()),
        }
    }
}

impl<M, T: ?Sized> Debug for SynchronizerUnsized<M, T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        with_ops!(self: x => x.fmt(f))
    }
}

impl<M, T> SynchronizerUnsized<M, T> {
    pub fn new(metadata: M, sync_type: SynchronizerType, value: T) -> Self {
        match sync_type {
            SynchronizerType::Arc => Self::Arc(Arc::new(SynchronizerImpl {
                metadata,
                container: value,
            })),
            SynchronizerType::Rcu => unimplemented!("RCU must be called with new_cloneable"),
            SynchronizerType::Mutex => Self::Mutex(Arc::new(SynchronizerImpl {
                metadata,
                container: Mutex::new(value),
            })),
            SynchronizerType::RwLock => Self::RwLock(Arc::new(SynchronizerImpl {
                metadata,
                container: RwLock::new(value),
            })),
        }
    }

    pub fn new_cloneable(metadata: M, sync_type: SynchronizerType, value: T) -> Self
    where
        T: Clone,
    {
        match sync_type {
            SynchronizerType::Rcu => Self::ReadCopyUpdate(Arc::new(SynchronizerImpl {
                metadata,
                container: RcuLock::new(value),
            })),
            _ => Self::new(metadata, sync_type, value),
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

impl<M, T: ?Sized> SynchronizerUnsized<M, T> {
    pub fn lock_read(&self) -> SynchronizerReadLock<M, T> {
        with_ops!(self: x => x.lock_read())
    }
    pub fn try_lock_read(&self) -> Option<SynchronizerReadLock<M, T>> {
        with_ops!(self: x => x.try_lock_read())
    }
    pub fn lock_write(&self) -> SynchronizerWriteLock<M, T> {
        with_ops!(self: x => x.lock_write())
    }
    pub fn try_lock_write(&self) -> Option<SynchronizerWriteLock<M, T>> {
        with_ops!(self: x => x.try_lock_write())
    }
}

pub trait SynchronizerOps<M, T: ?Sized> {
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized;
    fn try_unwrap_or_sync(
        self,
        f: impl Fn(Arc<Self>) -> SynchronizerUnsized<M, T>,
    ) -> Result<T, SynchronizerUnsized<M, T>>
    where
        Self: Sized,
        T: Sized,
    {
        self.try_unwrap().map_err(|x| f(Arc::new(x)))
    }
    fn lock_read(&self) -> SynchronizerReadLock<M, T>;
    fn try_lock_read(&self) -> Option<SynchronizerReadLock<M, T>>;
    fn lock_write(&self) -> SynchronizerWriteLock<M, T>;
    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<M, T>>;
}

pub struct SynchronizerImpl<M, C: ?Sized> {
    metadata: M,
    container: C,
}

impl<M, C: ?Sized> Debug for SynchronizerImpl<M, C>
where
    C: Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.container.fmt(f)
    }
}

impl<M, T: ?Sized> SynchronizerOps<M, T> for SynchronizerImpl<M, RwLock<T>> {
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        Ok(self.container.into_inner())
    }

    fn lock_read(&self) -> SynchronizerReadLock<M, T> {
        SynchronizerReadLock::RwLock(&self.metadata, self.container.read())
    }

    fn try_lock_read(&self) -> Option<SynchronizerReadLock<M, T>> {
        self.container
            .try_read()
            .map(|x| SynchronizerReadLock::RwLock(&self.metadata, x))
    }

    fn lock_write(&self) -> SynchronizerWriteLock<M, T> {
        SynchronizerWriteLock::RwLock(&self.metadata, self.container.write())
    }

    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<M, T>> {
        self.container
            .try_write()
            .map(|x| SynchronizerWriteLock::RwLock(&self.metadata, x))
    }
}

impl<M, T: ?Sized> SynchronizerOps<M, T> for SynchronizerImpl<M, Mutex<T>> {
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        Ok(self.container.into_inner())
    }

    fn lock_read(&self) -> SynchronizerReadLock<M, T> {
        SynchronizerReadLock::Mutex(&self.metadata, self.container.lock())
    }

    fn try_lock_read(&self) -> Option<SynchronizerReadLock<M, T>> {
        self.container
            .try_lock()
            .map(|x| SynchronizerReadLock::Mutex(&self.metadata, x))
    }

    fn lock_write(&self) -> SynchronizerWriteLock<M, T> {
        SynchronizerWriteLock::Mutex(&self.metadata, self.container.lock())
    }

    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<M, T>> {
        self.container
            .try_lock()
            .map(|x| SynchronizerWriteLock::Mutex(&self.metadata, x))
    }
}

impl<M, T: ?Sized> SynchronizerOps<M, T> for SynchronizerImpl<M, RcuLock<T>> {
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        let metadata = self.metadata;
        self.container.try_unwrap().map_err(|container| Self {
            metadata,
            container,
        })
    }

    fn lock_read(&self) -> SynchronizerReadLock<M, T> {
        SynchronizerReadLock::ReadCopyUpdate(&self.metadata, self.container.read())
    }

    fn try_lock_read(&self) -> Option<SynchronizerReadLock<M, T>> {
        Some(self.lock_read())
    }

    fn lock_write(&self) -> SynchronizerWriteLock<M, T> {
        SynchronizerWriteLock::ReadCopyUpdate(&self.metadata, self.container.write())
    }

    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<M, T>> {
        Some(self.lock_write())
    }
}

impl<M, T: ?Sized> SynchronizerOps<M, T> for SynchronizerImpl<M, T> {
    fn try_unwrap(self) -> Result<T, Self>
    where
        Self: Sized,
        T: Sized,
    {
        Ok(self.container)
    }

    fn lock_read(&self) -> SynchronizerReadLock<M, T> {
        SynchronizerReadLock::Arc(&self.metadata, &self.container)
    }

    fn try_lock_read(&self) -> Option<SynchronizerReadLock<M, T>> {
        Some(self.lock_read())
    }

    fn lock_write(&self) -> SynchronizerWriteLock<M, T> {
        unreachable!()
    }

    fn try_lock_write(&self) -> Option<SynchronizerWriteLock<M, T>> {
        unreachable!()
    }
}
