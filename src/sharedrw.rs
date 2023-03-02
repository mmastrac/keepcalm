use crate::locks::*;
use crate::projection::*;
use crate::Shared;
use std::sync::{Arc, Mutex, PoisonError, RwLock};

/// Determines what should happen if the underlying synchronization primitive is poisoned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Implementation {
    RwLock,
    Mutex,
}

enum SharedRWImpl<T: ?Sized> {
    /// Only usable by non-mutable shares.
    Arc(Arc<T>),
    RwLock(PoisonPolicy, Arc<RwLock<T>>),
    /// Used for unsized types
    RwLockBox(PoisonPolicy, Arc<RwLock<Box<T>>>),
    Mutex(PoisonPolicy, Arc<Mutex<T>>),
    Projection(Arc<dyn SharedRWProjection<T> + 'static>),
}

/// The [`SharedRW`] object hides the complexity of managing `Arc<Mutex<T>>` or `Arc<RwLock<T>>` behind a single interface:
///
/// ```rust
/// # use keepcalm::*;
/// let object = "123".to_string();
/// let shared = SharedRW::new(object);
/// shared.lock_read();
/// ```
///
/// By default, a [`SharedRW`] object uses `Arc<RwLock<T>>` under the hood, but you can choose the synchronization primitive at
/// construction time. The [`SharedRW`] object *erases* the underlying primitive and you can use them interchangeably:
///
/// ```rust
/// # use keepcalm::*;
/// fn use_shared(shared: SharedRW<String>) {
///     shared.lock_read();
/// }
///
/// let shared = SharedRW::new("123".to_string());
/// use_shared(shared);
/// let shared = SharedRW::new_with_type("123".to_string(), Implementation::Mutex);
/// use_shared(shared);
/// ```
///
/// Managing the poison state of synchronization primitives can be challenging as well. Rust will poison a `Mutex` or `RwLock` if you
/// hold a lock while a `panic!` occurs.
///
/// The `SharedRW` type allows you to specify a [`PoisonPolicy`] at construction time. By default, if a synchronization
/// primitive is poisoned, the `SharedRW` will `panic!` on access. This can be configured so that poisoning is ignored:
///
/// ```rust
/// # use keepcalm::*;
/// let shared = SharedRW::new_with_policy("123".to_string(), PoisonPolicy::Ignore);
/// ```
#[repr(transparent)]
pub struct SharedRW<T: ?Sized> {
    inner_impl: SharedRWImpl<T>,
}

// UNSAFETY: The construction and projection of SharedRW requires Send + Sync, so we can guarantee that
// all instances of SharedRW are Send + Sync.
unsafe impl<T: ?Sized> Send for SharedRW<T> {}
unsafe impl<T: ?Sized> Sync for SharedRW<T> {}

// UNSAFETY: Requires the caller to pass something that's Send + Sync in U to avoid unsafely constructing a SharedRW from a non-Send/non-Sync type.
fn make_shared_rw_value<U: Send + Sync, T: ?Sized + 'static>(
    inner_impl: SharedRWImpl<T>,
) -> SharedRW<T> {
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
                SharedRWImpl::Arc(x) => SharedRWImpl::Arc(x.clone()),
                SharedRWImpl::RwLock(policy, x) => SharedRWImpl::RwLock(*policy, x.clone()),
                SharedRWImpl::RwLockBox(policy, x) => SharedRWImpl::RwLockBox(*policy, x.clone()),
                SharedRWImpl::Mutex(policy, x) => SharedRWImpl::Mutex(*policy, x.clone()),
                SharedRWImpl::Projection(x) => SharedRWImpl::Projection(x.clone()),
            },
        }
    }
}

impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for SharedRWImpl<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SharedRWImpl::Arc(x) => x.fmt(f),
            SharedRWImpl::Mutex(policy, x) => f.write_fmt(format_args!("{:?} {:?}", policy, x)),
            SharedRWImpl::RwLock(policy, x) => f.write_fmt(format_args!("{:?} {:?}", policy, x)),
            SharedRWImpl::RwLockBox(policy, x) => f.write_fmt(format_args!("{:?} {:?}", policy, x)),
            // TODO: We should format the underlying projection
            SharedRWImpl::Projection(_x) => f.write_fmt(format_args!("(projection)")),
        }
    }
}

impl<T> SharedRWImpl<T> {
    /// Attempt to unwrap this synchronized object if we are the only holder of its value.
    fn try_unwrap(self) -> Result<T, Self> {
        match self {
            SharedRWImpl::Arc(x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(x),
                Err(x) => Err(SharedRWImpl::Arc(x)),
            },
            SharedRWImpl::Mutex(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(policy.handle_lock(x.into_inner())),
                Err(x) => Err(SharedRWImpl::Mutex(policy, x)),
            },
            SharedRWImpl::RwLock(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(policy.handle_lock(x.into_inner())),
                Err(x) => Err(SharedRWImpl::RwLock(policy, x)),
            },
            SharedRWImpl::RwLockBox(policy, x) => match Arc::try_unwrap(x) {
                Ok(x) => Ok(*policy.handle_lock(x.into_inner())),
                Err(x) => Err(SharedRWImpl::RwLockBox(policy, x)),
            },
            SharedRWImpl::Projection(_) => Err(self),
        }
    }
}

impl<T: ?Sized> SharedRWImpl<T> {
    pub fn lock_read(&self) -> SharedReadLock<T> {
        match &self {
            SharedRWImpl::Arc(x) => SharedReadLock {
                inner: SharedReadLockInner::Arc(&x),
            },
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
        match &self {
            SharedRWImpl::Arc(x) => unreachable!("This should not be possible"),
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

impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for SharedRW<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner_impl.fmt(f)
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

#[cfg(feature = "serde")]
use serde::Serialize;

#[cfg(feature = "serde")]
impl<'a, T: Serialize> Serialize for SharedRW<T>
where
    &'a T: Serialize + 'static,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.lock_read().serialize(serializer)
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

    /// Attempt to unwrap this synchronized object if we are the only holder of its value.
    pub fn try_unwrap(self) -> Result<T, Self> {
        match self.inner_impl.try_unwrap() {
            Ok(x) => Ok(x),
            Err(x) => Err(make_shared_rw_value::<T, T>(x)),
        }
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
        self.inner_impl.lock_read()
    }

    pub fn lock_write(&self) -> SharedWriteLock<T> {
        self.inner_impl.lock_write()
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

    #[test]
    pub fn test_unwrap() {
        let shared = SharedRW::new(1);
        let res = shared.try_unwrap().expect("Expected to unwrap");
        assert_eq!(res, 1);

        let shared = SharedRW::new(1);
        let shared2 = shared.clone();
        // We can't unwrap with multiple references
        assert!(matches!(shared.try_unwrap(), Err(_)));
        // We can now unwrap
        assert!(matches!(shared2.try_unwrap(), Ok(_)));
    }

    #[cfg(feature = "serde")]
    #[test]
    pub fn test_serde() {
        fn serialize<S: serde::Serialize>(_: S) {}
        let shared = SharedRW::new((1, 2, 3));
        serialize(shared);
    }
}
