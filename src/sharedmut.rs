use crate::implementation::*;
use crate::locks::*;
use crate::projection::*;
use crate::Shared;
use parking_lot::{Mutex, RwLock};
use std::sync::Arc;

/// Specifies the underlying synchronization primitive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Implementation {
    RwLock,
    Mutex,
}

/// The [`SharedMut`] object hides the complexity of managing `Arc<Mutex<T>>` or `Arc<RwLock<T>>` behind a single interface:
///
/// ```rust
/// # use keepcalm::*;
/// let object = "123".to_string();
/// let shared = SharedMut::new(object);
/// shared.read();
/// ```
///
/// By default, a [`SharedMut`] object uses `Arc<RwLock<T>>` under the hood, but you can choose the synchronization primitive at
/// construction time. The [`SharedMut`] object *erases* the underlying primitive and you can use them interchangeably:
///
/// ```rust
/// # use keepcalm::*;
/// fn use_shared(shared: SharedMut<String>) {
///     shared.read();
/// }
///
/// let shared = SharedMut::new("123".to_string());
/// use_shared(shared);
/// let shared = SharedMut::new_with_type("123".to_string(), Implementation::Mutex);
/// use_shared(shared);
/// ```
///
/// Managing the poison state of synchronization primitives can be challenging as well. Rust will poison a `Mutex` or `RwLock` if you
/// hold a lock while a `panic!` occurs.
///
/// The `SharedMut` type allows you to specify a [`PoisonPolicy`] at construction time. By default, if a synchronization
/// primitive is poisoned, the `SharedMut` will `panic!` on access. This can be configured so that poisoning is ignored:
///
/// ```rust
/// # use keepcalm::*;
/// let shared = SharedMut::new_with_policy("123".to_string(), PoisonPolicy::Ignore);
/// ```
#[repr(transparent)]
pub struct SharedMut<T: ?Sized> {
    pub(crate) inner_impl: SharedImpl<T>,
}

// UNSAFETY: The construction and projection of SharedMut requires Send + Sync, so we can guarantee that
// all instances of SharedMut are Send + Sync.
unsafe impl<T: ?Sized> Send for SharedMut<T> {}
unsafe impl<T: ?Sized> Sync for SharedMut<T> {}

// UNSAFETY: Requires the caller to pass something that's Send + Sync in U to avoid unsafely constructing a SharedMut from a non-Send/non-Sync type.
fn make_shared_rw_value<U: Send + Sync, T: ?Sized>(inner_impl: SharedImpl<T>) -> SharedMut<T> {
    SharedMut { inner_impl }
}

// UNSAFETY: Projections are always Send + Sync safe.
fn make_shared_rw_projection<T: ?Sized>(inner_impl: SharedImpl<T>) -> SharedMut<T> {
    SharedMut { inner_impl }
}

// UNSAFETY: Safe to clone an object that is considered safe.
impl<T: ?Sized> Clone for SharedMut<T> {
    fn clone(&self) -> Self {
        Self {
            inner_impl: self.inner_impl.clone(),
        }
    }
}

impl<T: ?Sized + std::fmt::Debug> std::fmt::Debug for SharedMut<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.inner_impl.fmt(f)
    }
}

impl<T: ?Sized> SharedMut<T> {
    pub fn from_box(value: Box<T>) -> Self
    where
        Box<T>: Send + Sync + 'static,
    {
        make_shared_rw_value::<Box<T>, T>(SharedImpl::RwLockBox(
            PoisonPolicy::Panic,
            Arc::new(RwLock::new(value)),
        ))
    }
}

#[cfg(feature = "serde")]
use serde::Serialize;

#[cfg(feature = "serde")]
impl<'a, T: Serialize> Serialize for SharedMut<T>
where
    &'a T: Serialize + 'static,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.read().serialize(serializer)
    }
}

impl<T: ?Sized, P: ?Sized> SharedMutProjection<P> for (SharedMut<T>, ProjectorRW<T, P>) {
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
            lock: self.0.read(),
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
            lock: self.0.write(),
            projector: &self.1,
        };

        SharedWriteLock {
            inner: SharedWriteLockInner::Projection(Box::new(lock)),
        }
    }
}

// UNSAFETY: All construction functions are gated behind T: Send + Sync to ensure that we cannot
// construct a SharedMut without the underlying object being thread-safe.
impl<T: Send + Sync + 'static> SharedMut<T> {
    pub fn new(t: T) -> SharedMut<T> {
        make_shared_rw_value::<T, T>(SharedImpl::RwLock(
            PoisonPolicy::Panic,
            Arc::new(RwLock::new(t)),
        ))
    }

    pub fn new_with_type(t: T, implementation: Implementation) -> Self {
        make_shared_rw_value::<T, T>(match implementation {
            Implementation::Mutex => {
                SharedImpl::Mutex(PoisonPolicy::Panic, Arc::new(Mutex::new(t)))
            }
            Implementation::RwLock => {
                SharedImpl::RwLock(PoisonPolicy::Panic, Arc::new(RwLock::new(t)))
            }
        })
    }

    pub fn new_with_policy(t: T, policy: PoisonPolicy) -> Self {
        make_shared_rw_value::<T, T>(SharedImpl::RwLock(policy, Arc::new(RwLock::new(t))))
    }
}

impl<T> SharedMut<T> {
    /// Attempt to unwrap this synchronized object if we are the only holder of its value.
    pub fn try_unwrap(self) -> Result<T, Self> {
        match self.inner_impl.try_unwrap() {
            Ok(x) => Ok(x),
            Err(inner_impl) => Err(Self { inner_impl }),
        }
    }
}

impl<T: ?Sized> SharedMut<T> {
    pub fn project<P: ?Sized + 'static, I: Into<ProjectorRW<T, P>>>(
        &self,
        projector: I,
    ) -> SharedMut<P>
    where
        T: 'static,
    {
        let projectable = Arc::new((self.clone(), projector.into()));
        make_shared_rw_projection(SharedImpl::Projection(projectable))
    }

    pub fn project_fn<
        P: ?Sized + 'static,
        RO: (Fn(&T) -> &P) + Send + Sync + 'static,
        RW: (Fn(&mut T) -> &mut P) + Send + Sync + 'static,
    >(
        &self,
        ro: RO,
        rw: RW,
    ) -> SharedMut<P>
    where
        T: 'static,
    {
        let projectable = Arc::new((self.clone(), ProjectorRW::new(ro, rw)));
        make_shared_rw_projection(SharedImpl::Projection(projectable))
    }

    /// Transmutate this [`SharedMut`] into a [`Shared`]. The underlying lock stays the same.
    pub fn shared_copy(&self) -> Shared<T> {
        Shared::from(self.inner_impl.clone())
    }

    /// Consume and transmutate this [`SharedMut`] into a [`Shared`]. The underlying lock may be optimized if
    /// there are no other outstanding writeable references.
    pub fn shared(self) -> Shared<T> {
        Shared::from(self.inner_impl)
    }

    pub fn read(&self) -> SharedReadLock<T> {
        self.inner_impl.lock_read()
    }

    pub fn write(&self) -> SharedWriteLock<T> {
        self.inner_impl.lock_write()
    }
}

#[cfg(test)]
mod test {
    use crate::{project, project_cast};

    use super::{Implementation, SharedMut};

    #[test]
    pub fn test_shared_rw() {
        let shared = SharedMut::new(1);
        *shared.write() += 1;
        assert_eq!(*shared.read(), 2);
    }

    #[test]
    pub fn test_shared_rw_mutex() {
        let shared = SharedMut::new_with_type(1, Implementation::Mutex);
        *shared.write() += 1;
        assert_eq!(*shared.read(), 2);
    }

    #[test]
    pub fn test_shared_rw_projection() {
        let shared = SharedMut::new((1, 1));
        let shared_1 = shared.project_fn(|x| &x.0, |x| &mut (x.0));
        let shared_2 = shared.project_fn(|x| &x.1, |x| &mut (x.1));

        *shared_1.write() += 1;
        *shared_2.write() += 10;

        assert_eq!(*shared.read(), (2, 11));
    }

    #[test]
    pub fn test_nested_rw_projection() {
        let shared = SharedMut::new((1, (2, (3, 4))));

        let projection = project!(x: (i32, (i32, (i32, i32))), x.1);
        let shared2 = shared.project(projection);
        let projection2 = project!(x: (i32, (i32, i32)), x.1 .1);
        let shared3 = shared2.project(projection2);
        *shared3.write() += 10;
        (shared2.write().0) += 100;

        assert_eq!(*shared.read(), (1, (102, (3, 14))));
    }

    #[test]
    pub fn test_unsized_rw() {
        let shared =
            SharedMut::new("123".to_owned()).project(project_cast!(x: String => dyn AsRef<str>));
        assert_eq!(shared.read().as_ref(), "123");

        let boxed = Box::new([1, 2, 3]) as Box<[i32]>;
        let shared: SharedMut<[i32]> = SharedMut::from_box(boxed);
        assert_eq!(shared.read()[0], 1);
        shared.write()[0] += 10;
        assert_eq!(shared.read()[0], 11);
    }

    #[test]
    pub fn test_unwrap() {
        let shared = SharedMut::new(1);
        let res = shared.try_unwrap().expect("Expected to unwrap");
        assert_eq!(res, 1);

        let shared = SharedMut::new(1);
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
        let shared = SharedMut::new((1, 2, 3));
        serialize(shared);
    }
}
