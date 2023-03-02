use crate::projection::*;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Determines what should happen if the underlying synchronization primitive is poisoned.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PoisonPolicy {
    Ignore,
    Panic,
}

impl PoisonPolicy {
    fn handle<T>(&self, error: PoisonError<T>) -> T {
        match self {
            PoisonPolicy::Ignore => error.into_inner(),
            PoisonPolicy::Panic => panic!("This shared object was poisoned"),
        }
    }

    fn handle_lock<T>(&self, res: Result<T, PoisonError<T>>) -> T {
        match res {
            Ok(lock) => lock,
            Err(err) => self.handle(err),
        }
    }
}

/// Specifies the underlying synchronization primitive.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Implementation {
    RwLock,
    Mutex,
}

enum SharedRWImpl<T: ?Sized> {
    RwLock(PoisonPolicy, Arc<RwLock<T>>),
    /// Used for unsized types
    RwLockBox(PoisonPolicy, Arc<RwLock<Box<T>>>),
    Mutex(PoisonPolicy, Arc<Mutex<T>>),
    Projection(Arc<dyn SharedRWProjection<T> + 'static>),
}

#[repr(transparent)]
pub struct SharedRW<T: ?Sized> {
    inner_impl: SharedRWImpl<T>,
}

// UNSAFETY: The construction and projection of SharedRW requires Send + Sync, so we can guarantee that
// all instances of SharedRW are Send + Sync.
unsafe impl<T: ?Sized> Send for SharedRW<T> {}
unsafe impl<T: ?Sized> Sync for SharedRW<T> {}

// UNSAFETY: Requires the caller to pass something that's Send + Sync in U to avoid unsafely constructing a SharedRW from a non-Send/non-Sync type.
fn make_shared_rw_value<U: Send + Sync, T: ?Sized + 'static>(inner_impl: SharedRWImpl<T>) -> SharedRW<T> {
    SharedRW { inner_impl }
}

// UNSAFETY: Projections are always Send + Sync safe.
fn make_shared_rw_projection<T: ?Sized>(inner_impl: SharedRWImpl<T>) -> SharedRW<T> {
    SharedRW { inner_impl }
}

// UNSAFETY: Safe to clone an object that is considered safe.
impl<T: ?Sized> Clone for SharedRW<T> {
    fn clone(&self) -> Self {
        Self {
            inner_impl: match &self.inner_impl {
                SharedRWImpl::RwLock(policy, x) => SharedRWImpl::RwLock(*policy, x.clone()),
                SharedRWImpl::RwLockBox(policy, x) => SharedRWImpl::RwLockBox(*policy, x.clone()),
                SharedRWImpl::Mutex(policy, x) => SharedRWImpl::Mutex(*policy, x.clone()),
                SharedRWImpl::Projection(x) => SharedRWImpl::Projection(x.clone()),
            },
        }
    }
}

impl<T: ?Sized> SharedRW<T> {
    pub fn from_box(value: Box<T>) -> Self
    where
        Box<T>: Send + Sync + 'static,
    {
        make_shared_rw_value::<Box<T>, T>(SharedRWImpl::RwLockBox(
            PoisonPolicy::Panic,
            Arc::new(RwLock::new(value)),
        ))
    }
}

trait SharedRWProjection<T: ?Sized>: Send + Sync {
    fn lock_read(&self) -> SharedReadLock<T>;
    fn lock_write(&self) -> SharedWriteLock<T>;
}

impl<T: ?Sized, P: ?Sized> SharedRWProjection<P> for (SharedRW<T>, ProjectorRW<T, P>) {
    fn lock_read(&self) -> SharedReadLock<P> {
        struct HiddenLock<'a, T: ?Sized, P: ?Sized> {
            lock: SharedReadLock<'a, T>,
            projector: &'a ProjectorRW<T, P>,
        }

        impl<'a, T: ?Sized, P: ?Sized> std::ops::Deref for HiddenLock<'a, T, P> {
            type Target = P;
            fn deref(&self) -> &Self::Target {
                (self.projector).project(&*self.lock)
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

    fn lock_write(&self) -> SharedWriteLock<P> {
        struct HiddenLock<'a, T: ?Sized, P: ?Sized> {
            lock: SharedWriteLock<'a, T>,
            projector: &'a ProjectorRW<T, P>,
        }

        impl<'a, T: ?Sized, P: ?Sized> std::ops::Deref for HiddenLock<'a, T, P> {
            type Target = P;
            fn deref(&self) -> &Self::Target {
                (self.projector).project(&*self.lock)
            }
        }

        impl<'a, T: ?Sized, P: ?Sized> std::ops::DerefMut for HiddenLock<'a, T, P> {
            fn deref_mut(&mut self) -> &mut Self::Target {
                (self.projector).project_mut(&mut *self.lock)
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

enum SharedReadLockInner<'a, T: ?Sized> {
    RwLock(RwLockReadGuard<'a, T>),
    RwLockBox(RwLockReadGuard<'a, Box<T>>),
    Mutex(MutexGuard<'a, T>),
    Projection(Box<dyn std::ops::Deref<Target = T> + 'a>),
}

pub struct SharedReadLock<'a, T: ?Sized> {
    inner: SharedReadLockInner<'a, T>,
}

pub enum SharedWriteLockInner<'a, T: ?Sized> {
    RwLock(RwLockWriteGuard<'a, T>),
    RwLockBox(RwLockWriteGuard<'a, Box<T>>),
    Mutex(MutexGuard<'a, T>),
    Projection(Box<dyn std::ops::DerefMut<Target = T> + 'a>),
}

pub struct SharedWriteLock<'a, T: ?Sized> {
    inner: SharedWriteLockInner<'a, T>,
}

impl<'a, T: ?Sized> std::ops::Deref for SharedReadLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        use SharedReadLockInner::*;
        match &self.inner {
            RwLock(x) => x,
            RwLockBox(x) => x,
            Mutex(x) => x,
            Projection(x) => x,
        }
    }
}

impl<'a, T: ?Sized> std::ops::Deref for SharedWriteLock<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        use SharedWriteLockInner::*;
        match &self.inner {
            RwLock(x) => x,
            RwLockBox(x) => x,
            Mutex(x) => x,
            Projection(x) => x,
        }
    }
}

impl<'a, T: ?Sized> std::ops::DerefMut for SharedWriteLock<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        use SharedWriteLockInner::*;
        match &mut self.inner {
            RwLock(x) => &mut *x,
            RwLockBox(x) => &mut *x,
            Mutex(x) => &mut *x,
            Projection(x) => &mut *x,
        }
    }
}

// UNSAFETY: All construction functions are gated behind T: Send + Sync to ensure that we cannot
// construct a SharedRW without the underlying object being thread-safe.
impl<T: Send + Sync + 'static> SharedRW<T> {
    pub fn new(t: T) -> SharedRW<T> {
        make_shared_rw_value::<T, T>(SharedRWImpl::RwLock(
            PoisonPolicy::Panic,
            Arc::new(RwLock::new(t)),
        ))
    }

    pub fn new_with_type(t: T, implementation: Implementation) -> Self {
        make_shared_rw_value::<T, T>(match implementation {
            Implementation::Mutex => {
                SharedRWImpl::Mutex(PoisonPolicy::Panic, Arc::new(Mutex::new(t)))
            }
            Implementation::RwLock => {
                SharedRWImpl::RwLock(PoisonPolicy::Panic, Arc::new(RwLock::new(t)))
            }
        })
    }

    pub fn new_with_policy(t: T, policy: PoisonPolicy) -> Self {
        make_shared_rw_value::<T, T>(SharedRWImpl::RwLock(policy, Arc::new(RwLock::new(t))))
    }
}

impl<T: ?Sized> SharedRW<T> {
    pub fn project<P: ?Sized + 'static, I: Into<ProjectorRW<T, P>>>(
        &self,
        projector: I,
    ) -> SharedRW<P>
    where
        T: 'static,
    {
        let projectable = Arc::new((self.clone(), projector.into()));
        make_shared_rw_projection(SharedRWImpl::Projection(projectable))
    }

    pub fn project_fn<
        P: ?Sized + 'static,
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
        make_shared_rw_projection(SharedRWImpl::Projection(projectable))
    }

    pub fn lock_read(&self) -> SharedReadLock<T> {
        match &self.inner_impl {
            SharedRWImpl::RwLock(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::RwLock(policy.handle_lock(lock.read())),
            },
            SharedRWImpl::RwLockBox(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::RwLockBox(policy.handle_lock(lock.read())),
            },
            SharedRWImpl::Mutex(policy, lock) => SharedReadLock {
                inner: SharedReadLockInner::Mutex(policy.handle_lock(lock.lock())),
            },
            SharedRWImpl::Projection(p) => p.lock_read(),
        }
    }

    pub fn lock_write(&self) -> SharedWriteLock<T> {
        match &self.inner_impl {
            SharedRWImpl::RwLock(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLock(policy.handle_lock(lock.write())),
            },
            SharedRWImpl::RwLockBox(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::RwLockBox(policy.handle_lock(lock.write())),
            },
            SharedRWImpl::Mutex(policy, lock) => SharedWriteLock {
                inner: SharedWriteLockInner::Mutex(policy.handle_lock(lock.lock())),
            },
            SharedRWImpl::Projection(p) => p.lock_write(),
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{project, project_cast};

    use super::{Implementation, SharedRW};

    #[test]
    pub fn test_shared_rw() {
        let shared = SharedRW::new(1);
        *shared.lock_write() += 1;
        assert_eq!(*shared.lock_read(), 2);
    }

    #[test]
    pub fn test_shared_rw_mutex() {
        let shared = SharedRW::new_with_type(1, Implementation::Mutex);
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

    #[test]
    pub fn test_nested_rw_projection() {
        let shared = SharedRW::new((1, (2, (3, 4))));

        let projection = project!(x: (i32, (i32, (i32, i32))), x.1);
        let shared2 = shared.project(projection);
        let projection2 = project!(x: (i32, (i32, i32)), x.1 .1);
        let shared3 = shared2.project(projection2);
        *shared3.lock_write() += 10;
        (shared2.lock_write().0) += 100;

        assert_eq!(*shared.lock_read(), (1, (102, (3, 14))));
    }

    #[test]
    pub fn test_unsized_rw() {
        let shared =
            SharedRW::new("123".to_owned()).project(project_cast!(x: String => dyn AsRef<str>));
        assert_eq!(shared.lock_read().as_ref(), "123");

        let boxed = Box::new([1, 2, 3]) as Box<[i32]>;
        let shared: SharedRW<[i32]> = SharedRW::from_box(boxed);
        assert_eq!(shared.lock_read()[0], 1);
        shared.lock_write()[0] += 10;
        assert_eq!(shared.lock_read()[0], 11);
    }
}
