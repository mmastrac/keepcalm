use crate::implementation::{
    make_shared_arc, make_shared_arc_box, make_shared_mutex, SharedImpl, SharedProjection,
};
use crate::locks::{SharedReadLock, SharedReadLockInner};
use crate::projection::Projector;
use crate::{PoisonPolicy, SharedMut};
use std::sync::Arc;

/// The default [`Shared`] object is similar to Rust's [`std::sync::Arc`], but adds the ability to project. [`Shared`] objects may also be
/// constructed as a `Mutex`, or may be a read-only view into a [`SharedMut`].
///
/// Note that because of this flexibility, the [`Shared`] object is slightly more complex than a traditional [`std::sync::Arc`], as all accesses
/// must be performed through the [`Shared::read`] accessor.
#[repr(transparent)]
pub struct Shared<T: ?Sized> {
    inner: SharedImpl<T>,
}

// UNSAFETY: The construction and projection of Shared requires Send + Sync, so we can guarantee that
// all instances of SharedMutImpl are Send + Sync.
unsafe impl<T: ?Sized> Send for Shared<T> {}
unsafe impl<T: ?Sized> Sync for Shared<T> {}

impl<T: ?Sized> std::panic::RefUnwindSafe for Shared<T> {}

#[cfg(feature = "serde")]
use serde::Serialize;

#[cfg(feature = "serde")]
impl<'a, T: Serialize> Serialize for Shared<T>
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

impl<T: ?Sized> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Shared<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.read().fmt(f)
    }
}

impl<T: ?Sized> From<Box<T>> for Shared<T>
where
    Box<T>: Send + Sync,
{
    fn from(value: Box<T>) -> Self {
        Self {
            inner: SharedImpl::ArcBox(Arc::new(make_shared_arc_box(value))),
        }
    }
}

// Use for transmutation from SharedMut to Shared.
impl<T: ?Sized> From<SharedImpl<T>> for Shared<T> {
    fn from(inner: SharedImpl<T>) -> Self {
        Self { inner }
    }
}

// Use for transmutation from SharedMut to Shared.
impl<T: ?Sized> From<SharedMut<T>> for Shared<T> {
    fn from(shared_rw: SharedMut<T>) -> Self {
        Self {
            inner: shared_rw.inner_impl,
        }
    }
}

impl<T: ?Sized> Shared<T> {
    pub fn from_box(t: Box<T>) -> Self
    where
        Box<T>: Send + Sync,
    {
        Self {
            inner: SharedImpl::ArcBox(Arc::new(make_shared_arc_box(t))),
        }
    }
}

impl<T: Send> Shared<T> {
    /// The [`Shared::new`] function requires a type that is both `Send + Sync`. If this is not possible, you may call
    /// [`Shared::new_unsync`] to create a [`Shared`] implementation that uses a [`Mutex`].
    ///
    /// This will fail to compile:
    ///
    /// ```compile_fail
    /// # use std::{cell::Cell, marker::PhantomData};
    /// # use keepcalm::*;
    /// # pub type Unsync = PhantomData<Cell<()>>;
    /// // Compiler error:
    /// // error[E0277]: (type) cannot be shared between threads safely
    /// // required by a bound in `keepcalm::Shared::<T>::new
    /// Shared::new(Unsync {});
    /// ```
    ///
    /// This will work:
    ///
    /// ```rust
    /// # use std::{cell::Cell, marker::PhantomData};
    /// # use keepcalm::*;
    /// # pub type Unsync = PhantomData<Cell<()>>;
    /// Shared::new_unsync(Unsync {});
    /// ```
    pub fn new_unsync(t: T) -> Self {
        Self::new_mutex(t)
    }

    /// Create a new [`Shared`], backed by a `Mutex` and poisoning on panic.
    pub fn new_mutex(t: T) -> Self {
        Self::new_mutex_with_policy(t, PoisonPolicy::Panic)
    }

    /// Create a new [`Shared`], backed by a `Mutex` and optionally poisoning on panic.
    pub fn new_mutex_with_policy(t: T, policy: PoisonPolicy) -> Self {
        Self {
            inner: SharedImpl::Mutex(Arc::new(make_shared_mutex(policy, t))),
        }
    }
}

impl<T: Send + Sync + 'static> Shared<T> {
    /// Create a new [`Shared`], backed by an `Arc` and poisoning on panic.
    pub fn new(t: T) -> Self {
        Self {
            inner: SharedImpl::Arc(Arc::new(make_shared_arc(t))),
        }
    }
}

impl<T> Shared<T> {
    /// Attempt to unwrap this object if we are the only holder of its value.
    pub fn try_unwrap(self) -> Result<T, Self> {
        match self.inner.try_unwrap() {
            Ok(x) => Ok(x),
            Err(inner) => Err(Self { inner }),
        }
    }
}

impl<T: ?Sized, P: ?Sized> SharedProjection<P> for (Shared<T>, Arc<Projector<T, P>>) {
    fn read(&self) -> SharedReadLock<P> {
        struct HiddenLock<'a, T: ?Sized, P: ?Sized> {
            lock: SharedReadLock<'a, T>,
            projector: &'a Projector<T, P>,
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
            poison: None,
        }
    }

    fn try_lock_read(&self) -> Option<SharedReadLock<P>> {
        let lock = self.0.try_read();
        if let Some(lock) = lock {
            struct HiddenLock<'a, T: ?Sized, P: ?Sized> {
                lock: SharedReadLock<'a, T>,
                projector: &'a Projector<T, P>,
            }

            impl<'a, T: ?Sized, P: ?Sized> std::ops::Deref for HiddenLock<'a, T, P> {
                type Target = P;
                fn deref(&self) -> &Self::Target {
                    (self.projector).project(&*self.lock)
                }
            }

            let lock = HiddenLock {
                lock,
                projector: &self.1,
            };

            Some(SharedReadLock {
                inner: SharedReadLockInner::Projection(Box::new(lock)),
                poison: None,
            })
        } else {
            None
        }
    }
}

impl<T: ?Sized> Shared<T> {
    pub fn project<P: ?Sized + 'static, I: Into<Projector<T, P>>>(&self, projector: I) -> Shared<P>
    where
        T: 'static,
    {
        let projector: Projector<T, P> = projector.into();
        let projectable = Arc::new((self.clone(), Arc::new(projector)));
        Shared {
            inner: SharedImpl::ProjectionRO(projectable),
        }
    }

    pub fn project_fn<P: ?Sized + 'static, RO: (Fn(&T) -> &P) + Send + Sync + 'static>(
        &self,
        ro: RO,
    ) -> Shared<P>
    where
        T: 'static,
    {
        let projectable = Arc::new((self.clone(), Arc::new(Projector::new(ro))));
        Shared {
            inner: SharedImpl::ProjectionRO(projectable),
        }
    }

    /// Get a read lock for this [`Shared`].
    pub fn read(&self) -> SharedReadLock<T> {
        self.inner.lock_read()
    }

    /// Try to get a read lock for this [`Shared`], or return [`None`] if we couldn't.
    pub fn try_read(&self) -> Option<SharedReadLock<T>> {
        self.inner.try_lock_read()
    }
}

#[cfg(test)]
mod test {
    use crate::project_cast;

    use super::*;

    #[allow(unused)]
    fn ensure_send<T: Send>() {}
    #[allow(unused)]
    fn ensure_sync<T: Sync>() {}

    #[allow(unused)]
    fn test_types() {
        ensure_send::<Shared<usize>>();
        ensure_sync::<Shared<usize>>();
        ensure_send::<Shared<dyn AsRef<str>>>();
        ensure_sync::<Shared<dyn AsRef<str>>>();
    }

    #[test]
    pub fn test_shared() {
        let shared = Shared::new(1);
        assert_eq!(shared.read(), 1);
    }

    #[test]
    pub fn test_shared_projection() {
        let shared = Shared::new((1, 2));
        let shared_proj = shared.project_fn(|x| &x.0);
        assert_eq!(shared_proj.read(), 1);
        let shared_proj = shared.project_fn(|x| &x.1);
        assert_eq!(shared_proj.read(), 2);
    }

    #[test]
    pub fn test_unsized() {
        let shared: Shared<dyn AsRef<str>> =
            Shared::new("123".to_owned()).project(project_cast!(x: String => dyn AsRef<str>));
        assert_eq!(shared.read().as_ref(), "123");

        let shared: Shared<[i32]> = Shared::from_box(Box::new([1, 2, 3]));
        assert_eq!(shared.read()[0], 1);
    }

    #[test]
    pub fn test_unwrap() {
        let shared = Shared::new(1);
        let res = shared.try_unwrap().expect("Expected to unwrap");
        assert_eq!(res, 1);

        let shared = Shared::new(1);
        let shared2 = shared.clone();
        // We can't unwrap with multiple references
        assert!(matches!(shared.try_unwrap(), Err(_)));
        // We can now unwrap
        assert!(matches!(shared2.try_unwrap(), Ok(_)));
    }

    #[test]
    pub fn test_unsync() {
        use std::{cell::Cell, marker::PhantomData};
        pub type Unsync = PhantomData<Cell<()>>;
        Shared::new_unsync(Unsync {});
    }

    #[cfg(feature = "serde")]
    #[test]
    pub fn test_serde() {
        fn serialize<S: serde::Serialize>(_: S) {}
        let shared = Shared::new((1, 2, 3));
        serialize(shared);
    }
}
