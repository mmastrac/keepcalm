use crate::implementation::*;
use crate::locks::*;
use crate::projection::*;
use crate::synchronizer::SynchronizerType;
use crate::synchronizer::SynchronizerUnsized;
use crate::Shared;
use std::sync::Arc;

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
/// let shared = SharedMut::new_mutex("123".to_string());
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

impl<T: ?Sized> std::panic::RefUnwindSafe for SharedMut<T> {}

// UNSAFETY: The construction and projection of SharedMut requires Send + Sync, so we can guarantee that
// all instances of SharedMut are Send + Sync.
unsafe impl<T: ?Sized> Send for SharedMut<T> {}
unsafe impl<T: ?Sized> Sync for SharedMut<T> {}

// UNSAFETY: Requires the caller to pass something that's Send + Sync in U to avoid unsafely constructing a SharedMut from a non-Send/non-Sync type.
fn make_shared_rw_value<T: ?Sized>(inner_impl: SharedImpl<T>) -> SharedMut<T> {
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
        make_shared_rw_value::<T>(SharedImpl::Box(SynchronizerUnsized::new(
            LockMetadata::poison_panic(),
            SynchronizerType::RwLock,
            value,
        )))
    }

    pub(crate) const fn from_inner(inner_impl: SharedImpl<T>) -> Self {
        Self { inner_impl }
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

    fn try_lock_read(&self) -> Option<SharedReadLock<P>> {
        let lock = self.0.try_read();
        if let Some(lock) = lock {
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
                lock,
                projector: &self.1,
            };

            Some(SharedReadLock {
                inner: SharedReadLockInner::Projection(Box::new(lock)),
            })
        } else {
            None
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

    fn try_lock_write(&self) -> Option<SharedWriteLock<P>> {
        let lock = self.0.try_write();
        if let Some(lock) = lock {
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
                lock,
                projector: &self.1,
            };

            Some(SharedWriteLock {
                inner: SharedWriteLockInner::Projection(Box::new(lock)),
            })
        } else {
            None
        }
    }
}

// UNSAFETY: All construction functions are gated behind T: Send + Sync to ensure that we cannot
// construct a SharedMut without the underlying object being thread-safe.
impl<T: Send + Sync + 'static> SharedMut<T> {
    /// Create a new [`SharedMut`], backed by a `RwLock` and poisoning on panic.
    pub fn new(t: T) -> SharedMut<T> {
        Self::new_with_type(t, ImplementationMut::RwLock)
    }

    /// Create a new [`SharedMut`], backed by a `Mutex` and poisoning on panic.
    pub fn new_mutex(t: T) -> SharedMut<T> {
        Self::new_with_type(t, ImplementationMut::Mutex)
    }

    fn new_with_type(t: T, implementation: ImplementationMut) -> Self {
        Self::new_with_type_and_policy(t, implementation, PoisonPolicy::Panic)
    }

    /// Create a new [`SharedMut`], backed by a `RwLock` and optionally poisoning on panic.
    pub fn new_with_policy(t: T, policy: PoisonPolicy) -> Self {
        Self::new_with_type_and_policy(t, ImplementationMut::RwLock, policy)
    }

    /// Create a new [`SharedMut`], backed by a `Mutex` and optionally poisoning on panic.
    pub fn new_mutex_with_policy(t: T, policy: PoisonPolicy) -> Self {
        Self::new_with_type_and_policy(t, ImplementationMut::Mutex, policy)
    }

    fn new_with_type_and_policy(
        t: T,
        implementation: ImplementationMut,
        policy: PoisonPolicy,
    ) -> Self {
        let synchronizer_type = match implementation {
            ImplementationMut::Mutex => SynchronizerType::Mutex,
            ImplementationMut::Rcu => unimplemented!("Use SharedMut::new_rcu instead"),
            ImplementationMut::RwLock => SynchronizerType::RwLock,
        };

        make_shared_rw_value::<T>(SharedImpl::Value(SynchronizerUnsized::new(
            LockMetadata::poison_policy(policy),
            synchronizer_type,
            t,
        )))
    }
}

impl<T: Send + Sync + Clone + 'static> SharedMut<T> {
    /// Creates a [`SharedMut`] that updates only when the write lock is dropped. This style of synchronization trades off expensive write operations (one clone, plus
    /// the final memory copy on commit) for extremely short lock times. In addition, a steady volume of writes with a steady volume of reads could result in large
    /// memory usage as multiple copies of the object may be kept in memory.
    ///
    /// Neither readers nor writers hold long-term locks on the underlying data, making the contention on this structure much lower than other styles.
    pub fn new_rcu(t: T) -> SharedMut<T> {
        Self::new_cloneable_with_type_and_policy(t, ImplementationMut::Rcu, PoisonPolicy::Ignore)
    }

    fn new_cloneable_with_type_and_policy(
        t: T,
        implementation: ImplementationMut,
        policy: PoisonPolicy,
    ) -> Self {
        let synchronizer_type = match implementation {
            ImplementationMut::Mutex => SynchronizerType::Mutex,
            ImplementationMut::Rcu => SynchronizerType::Rcu,
            ImplementationMut::RwLock => SynchronizerType::RwLock,
        };

        make_shared_rw_value::<T>(SharedImpl::Value(SynchronizerUnsized::new_cloneable(
            LockMetadata::poison_policy(policy),
            synchronizer_type,
            t,
        )))
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

    /// Get a read lock for this [`SharedMut`].
    pub fn read(&self) -> SharedReadLock<T> {
        self.inner_impl.lock_read()
    }

    /// Try to get a read lock for this [`SharedMut`], or return [`None`] if we couldn't.
    pub fn try_read(&self) -> Option<SharedReadLock<T>> {
        self.inner_impl.try_lock_read()
    }

    #[allow(unused)]
    pub(crate) fn read_owned(&self) -> SharedReadLockOwned<T> {
        self.inner_impl.lock_read_owned()
    }

    /// Get a write lock for this [`SharedMut`].
    pub fn write(&self) -> SharedWriteLock<T> {
        self.inner_impl.lock_write()
    }

    /// Try to get a write lock for this [`SharedMut`], or return [`None`] if we couldn't.
    pub fn try_write(&self) -> Option<SharedWriteLock<T>> {
        self.inner_impl.try_lock_write()
    }

    /// Atomically set the value of this [`SharedMut`] to a new value.
    pub fn set(&self, t: T)
    where
        T: Sized,
    {
        *self.write() = t;
    }

    /// Cast this [`SharedMut`] to a new type.
    ///
    /// See [`Castable`] for more information.
    pub fn cast<U>(&self) -> SharedMut<U>
    where
        T: Castable<U> + 'static,
        U: ?Sized + 'static,
    {
        let projector = crate::ProjectorRW {
            ro: Box::new(()),
            rw: Box::new(()),
        };
        let projectable = Arc::new((self.clone(), projector));
        make_shared_rw_projection(SharedImpl::Projection(projectable))
    }

    #[allow(unused)]
    pub(crate) fn write_owned(&self) -> SharedWriteLockOwned<T> {
        self.inner_impl.lock_write_owned()
    }

    #[cfg(feature = "async_experimental")]
    pub async fn read_async(&self, spawner: &crate::Spawner) -> SharedReadLock<T>
    where
        T: 'static,
    {
        if let Some(lock) = self.try_read() {
            return lock;
        }
        let lock = spawner
            .spawn_blocking_map(self.clone(), |lock| lock.read_owned())
            .await;
        return unsafe { lock.unsafe_reattach(&self.inner_impl) };
    }

    #[cfg(feature = "async_experimental")]
    pub async fn write_async(&self, spawner: &crate::Spawner) -> SharedWriteLock<T>
    where
        T: 'static,
    {
        if let Some(lock) = self.try_write() {
            return lock;
        }
        let lock = spawner
            .spawn_blocking_map(self.clone(), |lock| lock.write_owned())
            .await;
        return unsafe { lock.unsafe_reattach(&self.inner_impl) };
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{project, project_cast, PoisonPolicy};
    use paste::paste;

    #[test]
    pub fn test_shared_rw() {
        let shared = SharedMut::new(1);
        *shared.write() += 1;
        assert_eq!(shared.read(), 2);
    }

    #[test]
    pub fn test_shared_rw_mutex() {
        let shared = SharedMut::new_mutex(1);
        *shared.write() += 1;
        assert_eq!(shared.read(), 2);
    }

    #[test]
    pub fn test_shared_rw_projection() {
        let shared = SharedMut::new((1, 1));
        let shared_1 = shared.project_fn(|x| &x.0, |x| &mut (x.0));
        let shared_2 = shared.project_fn(|x| &x.1, |x| &mut (x.1));

        *shared_1.write() += 1;
        *shared_2.write() += 10;

        assert_eq!(shared.read(), (2, 11));
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
        _ = shared2.try_read().expect("Should be able to read");
        _ = shared2.try_write().expect("Should be able to write");
        _ = shared3.try_read().expect("Should be able to read");
        _ = shared3.try_write().expect("Should be able to write");

        assert_eq!(shared.read(), (1, (102, (3, 14))));
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
    pub fn test_unsized_slice_cast() {
        let shared: SharedMut<[i32]> = SharedMut::new([1, 2, 3]).cast();
        assert_eq!(shared.read().as_ref(), [1, 2, 3]);
    }

    #[test]
    pub fn test_unsized_rw_cast() {
        let shared: SharedMut<dyn AsRef<str>> = SharedMut::new("123".to_owned()).cast();
        assert_eq!(shared.read().as_ref(), "123");
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

    #[test]
    pub fn test_clone_commit() {
        let shared = SharedMut::new_rcu((1, 2));
        let read = shared.read();

        let mut write = shared.write();
        assert_eq!(write.0, 1);
        write.0 += 10;
        assert_eq!(write.0, 11);

        assert_eq!(read.0, 1);
        assert_eq!(shared.read().0, 1);
        drop(write);
        assert_eq!(read.0, 1);
        assert_eq!(shared.read().0, 11);
    }

    macro_rules! test_poison_policy {
        ($name:ident , $imp:ident , Ignore) => {
            paste! {
                #[test]
                pub fn [< $name _read >]() {
                    test_poison_policy!(= read, $imp, Ignore);
                }

                #[test]
                pub fn [< $name _write >]() {
                    test_poison_policy!(= write, $imp, Ignore);
                }
            }
        };
        ($name:ident , $imp:ident , Panic) => {
            paste! {
                #[test]
                #[should_panic]
                pub fn [< $name _read >]() {
                    test_poison_policy!(= read, $imp, Panic);
                }

                #[test]
                #[should_panic]
                pub fn [< $name _write >]() {
                    test_poison_policy!(= write, $imp, Panic);
                }
            }
        };
        (= $dir:ident, $imp:ident , $policy:ident) => {
            let shared = SharedMut::new_cloneable_with_type_and_policy(
                1_usize,
                ImplementationMut::$imp,
                PoisonPolicy::$policy,
            );
            let res = std::panic::catch_unwind(|| {
                let _lock = shared.$dir();
                panic!("This is a real panic, expect a backtrace!");
            });
            assert!(res.is_err());
            assert_eq!(shared.$dir(), 1);
        };
    }

    test_poison_policy!(test_poison_policy_mutex_ignore, Mutex, Ignore);
    test_poison_policy!(test_poison_policy_rwlock_ignore, RwLock, Ignore);
    test_poison_policy!(test_poison_policy_rcu_ignore, Rcu, Ignore);
    test_poison_policy!(test_poison_policy_mutex_panic, Mutex, Panic);
    test_poison_policy!(test_poison_policy_rwlock_panic, RwLock, Panic);
    test_poison_policy!(test_poison_policy_rcu_panic, Rcu, Panic);

    #[test]
    pub fn test_try_lock_mutex() {
        let shared = SharedMut::new_mutex(1);
        let read = shared.try_read().expect("Should get us a lock");
        assert!(shared.try_read().is_none());
        assert!(shared.try_write().is_none());
        drop(read);
    }

    #[test]
    pub fn test_try_lock_rwlock() {
        let shared = SharedMut::new(1);
        let read = shared.try_read().expect("Should get us a lock");
        assert!(shared.try_read().is_some());
        assert!(shared.try_write().is_none());
        drop(read);
        assert!(shared.try_write().is_some());
    }

    /// Test that RCU locks always succeed.
    #[test]
    pub fn test_try_lock_rcu() {
        let shared = SharedMut::new_rcu(1);
        let read = shared.try_read().expect("Should get us a lock");
        let write = shared.try_write().expect("Should get us a lock");
        assert!(shared.try_read().is_some());
        assert!(shared.try_write().is_some());
        drop(read);
        drop(write);
    }

    #[cfg(feature = "serde")]
    #[test]
    pub fn test_serde() {
        fn serialize<S: serde::Serialize>(_: S) {}
        let shared = SharedMut::new((1, 2, 3));
        serialize(shared);
    }

    #[test]
    pub fn test_rcu_and_projection() {
        let shared = SharedMut::new_rcu((1, 2, 3));
        let shared2 = shared.project(project!(x: (i32, i32, i32), x.0));
        assert_eq!(shared2.read(), 1);
        *shared2.write() = 10;
        assert_eq!(shared.read(), (10, 2, 3));
        assert_eq!(shared2.read(), 10);
    }
}
