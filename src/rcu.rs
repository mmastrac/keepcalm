use parking_lot::RwLock;
use std::{fmt::Debug, sync::Arc};

/// A read/copy/update "lock". Reads are nearly instantaneous as they work from the current version at the time of
/// the read. Writes are not blocked by reads.
pub struct RcuLock<T: ?Sized> {
    cloner: fn(&Arc<T>) -> Box<T>,
    lock: RwLock<Arc<T>>,
}

impl<T: ?Sized + Debug> Debug for RcuLock<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.lock.fmt(f)
    }
}

impl<T: ?Sized> RcuLock<T> {
    /// Create a new [`RcuLock`] from the given [`std::clone::Clone`]able value.
    pub fn new(value: T) -> Self
    where
        T: Clone + Sized,
    {
        Self {
            cloner: |x| Box::new((**x).clone()),
            lock: RwLock::new(Arc::new(value)),
        }
    }

    /// Take a read lock on this [`RcuLock`], which is basically just a clone of a reference the current value. This value will never change.
    pub fn read(&self) -> RcuReadGuard<T> {
        RcuReadGuard {
            value: self.lock.read().clone(),
        }
    }

    /// Take a write lock on this [`RcuLock`], which will commit to the current value of the [`RcuLock`] when the lock is dropped.
    pub fn write(&self) -> RcuWriteGuard<T> {
        RcuWriteGuard {
            lock: &self.lock,
            writer: Some((self.cloner)(&*self.lock.read())),
        }
    }

    pub fn try_unwrap(self) -> Result<T, Self>
    where
        T: Sized,
    {
        let lock = self.lock.into_inner();
        match Arc::try_unwrap(lock) {
            Ok(x) => Ok(x),
            Err(lock) => Err(Self {
                cloner: self.cloner,
                lock: RwLock::new(lock),
            }),
        }
    }
}

pub struct RcuReadGuard<T: ?Sized> {
    value: Arc<T>,
}

impl<T: ?Sized> std::ops::Deref for RcuReadGuard<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.value.deref()
    }
}

pub struct RcuWriteGuard<'a, T: ?Sized> {
    lock: &'a RwLock<Arc<T>>,
    writer: Option<Box<T>>,
}

impl<'a, T: ?Sized> std::ops::Deref for RcuWriteGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        self.writer.as_deref().expect("Cannot deref after drop")
    }
}

impl<'a, T: ?Sized> std::ops::DerefMut for RcuWriteGuard<'a, T> {
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
