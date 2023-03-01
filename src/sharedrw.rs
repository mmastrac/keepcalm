use crate::projection::*;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

const POISON_POLICY_IGNORE: u8 = (PoisonPolicy::Ignore) as u8;
const POISON_POLICY_PANIC: u8 = (PoisonPolicy::Panic) as u8;

enum PoisonPolicy {
    Ignore = 0,
    Panic = 1,
}

#[repr(transparent)]
pub struct SharedRW<T: Send + Sync, const POISON_POLICY: u8 = { POISON_POLICY_PANIC }> {
    inner: RawOrProjection<Arc<RwLock<T>>, BoxedProjection<T>>,
}

type BoxedProjection<T> = Arc<dyn SharedRWProjection<T> + Send + Sync>;

trait SharedRWProjection<T> {
    fn lock_read<'a>(&'a self) -> SharedReadLock<'a, T>;
    fn lock_write<'a>(&'a self) -> SharedWriteLock<'a, T>;
}

impl<T: Send + Sync, P: Send + Sync, const POISON_POLICY: u8> SharedRWProjection<P>
    for (SharedRW<T, POISON_POLICY>, ProjectorRW<T, P>)
{
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
            lock: RawOrProjection::Projection(Box::new(lock)),
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
            lock: RawOrProjection::Projection(Box::new(lock)),
        }
    }
}

impl<T: Send + Sync, const POISON_POLICY: u8> Clone for SharedRW<T, POISON_POLICY> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

#[repr(transparent)]
pub struct SharedReadLock<'a, T> {
    lock: RawOrProjection<RwLockReadGuard<'a, T>, Box<dyn std::ops::Deref<Target = T> + 'a>>,
}

#[repr(transparent)]
pub struct SharedWriteLock<'a, T> {
    lock: RawOrProjection<RwLockWriteGuard<'a, T>, Box<dyn std::ops::DerefMut<Target = T> + 'a>>,
}

impl<'a, T> std::ops::Deref for SharedReadLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        use RawOrProjection::*;
        match &self.lock {
            Raw(x) => x,
            Projection(x) => x,
        }
    }
}

impl<'a, T> std::ops::Deref for SharedWriteLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        use RawOrProjection::*;
        match &self.lock {
            Raw(x) => x,
            Projection(x) => x,
        }
    }
}

impl<'a, T> std::ops::DerefMut for SharedWriteLock<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        use RawOrProjection::*;
        match &mut self.lock {
            Raw(x) => &mut *x,
            Projection(x) => &mut *x,
        }
    }
}

impl<T: Send + Sync> SharedRW<T> {
    pub fn new(t: T) -> SharedRW<T, POISON_POLICY_PANIC> {
        SharedRW {
            inner: RawOrProjection::Raw(Arc::new(RwLock::new(t))),
        }
    }
}

impl<T: Send + Sync, const POISON_POLICY: u8> SharedRW<T, POISON_POLICY> {
    pub fn project<P: Send + Sync + 'static, I: Into<ProjectorRW<T, P>>>(
        &self,
        projector: I,
    ) -> SharedRW<P, POISON_POLICY>
    where
        T: 'static,
    {
        let projectable = Arc::new((self.clone(), projector.into()));
        SharedRW {
            inner: RawOrProjection::Projection(projectable),
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
    ) -> SharedRW<P, POISON_POLICY>
    where
        T: 'static,
    {
        let projectable = Arc::new((self.clone(), ProjectorRW::new(ro, rw)));
        SharedRW {
            inner: RawOrProjection::Projection(projectable),
        }
    }

    pub fn lock_read(&self) -> SharedReadLock<T> {
        match &self.inner {
            RawOrProjection::Raw(lock) => {
                let res = lock.read();
                let lock = match res {
                    Ok(lock) => lock,
                    Err(err) => err.into_inner(),
                };
                SharedReadLock {
                    lock: RawOrProjection::Raw(lock),
                }
            }
            RawOrProjection::Projection(p) => p.lock_read(),
        }
    }

    pub fn lock_write(&self) -> SharedWriteLock<T> {
        match &self.inner {
            RawOrProjection::Raw(lock) => {
                let res = lock.write();
                let lock = match res {
                    Ok(lock) => lock,
                    Err(err) => err.into_inner(),
                };
                SharedWriteLock {
                    lock: RawOrProjection::Raw(lock),
                }
            }
            RawOrProjection::Projection(p) => p.lock_write(),
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
