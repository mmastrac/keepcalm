use crate::projection::*;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

pub enum PoisonPolicy {
    Ignore = 0,
    Panic = 1,
}

enum SharedRWImpl<T> {
    RwLock(Arc<RwLock<T>>),
    Projection(Arc<dyn SharedRWProjection<T> + Send + Sync>),
}

#[repr(transparent)]
pub struct SharedRW<T: Send + Sync> {
    inner: SharedRWImpl<T>,
}

trait SharedRWProjection<T> {
    fn lock_read<'a>(&'a self) -> SharedReadLock<'a, T>;
    fn lock_write<'a>(&'a self) -> SharedWriteLock<'a, T>;
}

impl<T: Send + Sync, P: Send + Sync> SharedRWProjection<P> for (SharedRW<T>, ProjectorRW<T, P>) {
    fn lock_read<'a>(&'a self) -> SharedReadLock<'a, P> {
        struct HiddenLock<'a, T, P> {
            lock: SharedReadLock<'a, T>,
            projector: &'a ProjectorRW<T, P>,
        }

        impl<'a, T, P> std::ops::Deref for HiddenLock<'a, T, P> {
            type Target = P;
            fn deref(&self) -> &Self::Target {
                (self.projector.ro).project(&*self.lock)
            }
        }

        let lock = HiddenLock {
            lock: self.0.lock_read(),
            projector: &self.1,
        };

        SharedReadLock {
            inner: SharedReadLockInner::Projection(Box::new(lock)),
        }
    }

    fn lock_write<'a>(&'a self) -> SharedWriteLock<'a, P> {
        struct HiddenLock<'a, T, P> {
            lock: SharedWriteLock<'a, T>,
            projector: &'a ProjectorRW<T, P>,
        }

        impl<'a, T, P> std::ops::Deref for HiddenLock<'a, T, P> {
            type Target = P;
            fn deref(&self) -> &Self::Target {
                (self.projector.ro).project(&*self.lock)
            }
        }

        impl<'a, T, P> std::ops::DerefMut for HiddenLock<'a, T, P> {
            fn deref_mut(&mut self) -> &mut Self::Target {
                (self.projector.rw).project_mut(&mut *self.lock)
            }
        }

        let lock = HiddenLock {
            lock: self.0.lock_write(),
            projector: &self.1,
        };

        SharedWriteLock {
            inner: SharedWriteLockInner::Projection(Box::new(lock)),
        }
    }
}

impl<T: Send + Sync> Clone for SharedRW<T> {
    fn clone(&self) -> Self {
        Self {
            inner: match &self.inner {
                SharedRWImpl::RwLock(x) => SharedRWImpl::RwLock(x.clone()),
                SharedRWImpl::Projection(x) => SharedRWImpl::Projection(x.clone()),
            },
        }
    }
}

enum SharedReadLockInner<'a, T> {
    RwLock(RwLockReadGuard<'a, T>),
    Projection(Box<dyn std::ops::Deref<Target = T> + 'a>),
}

pub struct SharedReadLock<'a, T> {
    inner: SharedReadLockInner<'a, T>,
}

pub enum SharedWriteLockInner<'a, T> {
    RwLock(RwLockWriteGuard<'a, T>),
    Projection(Box<dyn std::ops::DerefMut<Target = T> + 'a>),
}

pub struct SharedWriteLock<'a, T> {
    inner: SharedWriteLockInner<'a, T>,
}

impl<'a, T> std::ops::Deref for SharedReadLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        use SharedReadLockInner::*;
        match &self.inner {
            RwLock(x) => x,
            Projection(x) => x,
        }
    }
}

impl<'a, T> std::ops::Deref for SharedWriteLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        use SharedWriteLockInner::*;
        match &self.inner {
            RwLock(x) => x,
            Projection(x) => x,
        }
    }
}

impl<'a, T> std::ops::DerefMut for SharedWriteLock<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        use SharedWriteLockInner::*;
        match &mut self.inner {
            RwLock(x) => &mut *x,
            Projection(x) => &mut *x,
        }
    }
}

impl<T: Send + Sync> SharedRW<T> {
    pub fn new(t: T) -> SharedRW<T> {
        SharedRW {
            inner: SharedRWImpl::RwLock(Arc::new(RwLock::new(t))),
        }
    }

    pub fn new_with_policy(t: T, policy: PoisonPolicy) -> Self {
        SharedRW {
            inner: SharedRWImpl::RwLock(Arc::new(RwLock::new(t))),
        }
    }

    pub fn project<P: Send + Sync + 'static, I: Into<ProjectorRW<T, P>>>(
        &self,
        projector: I,
    ) -> SharedRW<P>
    where
        T: 'static,
    {
        let projectable = Arc::new((self.clone(), projector.into()));
        SharedRW {
            inner: SharedRWImpl::Projection(projectable),
        }
    }

    pub fn project_fn<
        P: Send + Sync + 'static,
        RO: (Fn(&T) -> &P) + Send + Sync + 'static,
        RW: (Fn(&mut T) -> &mut P) + Send + Sync + 'static,
    >(
        &self,
        ro: RO,
        rw: RW,
    ) -> SharedRW<P>
    where
        T: 'static,
    {
        let projectable = Arc::new((self.clone(), ProjectorRW::new(ro, rw)));
        SharedRW {
            inner: SharedRWImpl::Projection(projectable),
        }
    }

    pub fn lock_read(&self) -> SharedReadLock<T> {
        match &self.inner {
            SharedRWImpl::RwLock(lock) => {
                let res = lock.read();
                let lock = match res {
                    Ok(lock) => lock,
                    Err(err) => err.into_inner(),
                };
                SharedReadLock {
                    inner: SharedReadLockInner::RwLock(lock),
                }
            }
            SharedRWImpl::Projection(p) => p.lock_read(),
        }
    }

    pub fn lock_write(&self) -> SharedWriteLock<T> {
        match &self.inner {
            SharedRWImpl::RwLock(lock) => {
                let res = lock.write();
                let lock = match res {
                    Ok(lock) => lock,
                    Err(err) => err.into_inner(),
                };
                SharedWriteLock {
                    inner: SharedWriteLockInner::RwLock(lock),
                }
            }
            SharedRWImpl::Projection(p) => p.lock_write(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::SharedRW;

    #[test]
    pub fn test_shared_rw() {
        let shared = SharedRW::new(1);
        *shared.lock_write() += 1;
        assert_eq!(*shared.lock_read(), 2);
    }

    #[test]
    pub fn test_shared_rw_projection() {
        let shared = SharedRW::new((1, 1));
        let shared_1 = shared.project_fn(|x| &x.0, |x| &mut (x.0));
        let shared_2 = shared.project_fn(|x| &x.1, |x| &mut (x.1));

        *shared_1.lock_write() += 1;
        *shared_2.lock_write() += 10;

        assert_eq!(*shared.lock_read(), (2, 11));
    }
}
