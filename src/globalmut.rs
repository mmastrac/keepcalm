use std::sync::{Arc, OnceLock};

use crate::{
    implementation::{
        LockMetadata, SharedGlobalImpl, SharedGlobalProjection, SharedImpl, SharedMutProjection,
    },
    synchronizer::{SynchronizerSized, SynchronizerType},
    Castable, PoisonPolicy, Shared, SharedMut, SharedReadLock, SharedWriteLock,
};

/// A global version of [`SharedMut`]. Use [`SharedGlobalMut::shared`] to get a [`Shared`] to access the contents, or [`SharedGlobalMut::shared_mut`] to
/// get a [`SharedMut`].
pub struct SharedGlobalMut<T: Send> {
    inner: SharedGlobalImpl<T>,
    projection: OnceLock<Arc<dyn SharedMutProjection<T>>>,
}

impl<T: Send + Sync> SharedGlobalMut<T> {
    /// Create a new [`SharedGlobalMut`].
    pub const fn new(t: T) -> Self {
        Self {
            inner: SharedGlobalImpl::Value(SynchronizerSized::new(
                LockMetadata::poison_policy(PoisonPolicy::Panic),
                SynchronizerType::RwLock,
                t,
            )),
            projection: OnceLock::new(),
        }
    }

    /// Create a new, lazy [`SharedGlobalMut`] implementation that will be initialized on the first access.
    pub const fn new_lazy(f: fn() -> T) -> Self {
        Self {
            inner: SharedGlobalImpl::lazy(f, SynchronizerType::RwLock, PoisonPolicy::Panic),
            projection: OnceLock::new(),
        }
    }
}

impl<T: Send> SharedGlobalMut<T> {
    /// The [`SharedGlobalMut::new`] function requires a type that is both `Send + Sync`. If this is not possible, you may call
    /// [`SharedGlobalMut::new_unsync`] to create a [`SharedGlobalMut`] implementation that uses a [`Mutex`].
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
    /// SharedGlobalMut::new(Unsync {});
    /// ```
    ///
    /// This will work:
    ///
    /// ```rust
    /// # use std::{cell::Cell, marker::PhantomData};
    /// # use keepcalm::*;
    /// # pub type Unsync = PhantomData<Cell<()>>;
    /// SharedGlobalMut::new_unsync(Unsync {});
    /// ```
    pub const fn new_unsync(t: T) -> Self {
        Self::new_mutex(t)
    }

    /// The [`SharedGlobalMut::new_lazy`] function requires a type that is both `Send + Sync`. If this is not possible, you may call
    /// [`SharedGlobalMut::new_lazy`] to create a [`SharedGlobalMut`] implementation that uses a [`Mutex`].
    pub const fn new_lazy_unsync(f: fn() -> T) -> Self {
        Self {
            inner: SharedGlobalImpl::lazy(f, SynchronizerType::Mutex, PoisonPolicy::Panic),
            projection: OnceLock::new(),
        }
    }

    /// Create a new [`SharedGlobalMut`], backed by a `Mutex` and poisoning on panic.
    pub const fn new_mutex(t: T) -> Self {
        Self::new_mutex_with_policy(t, PoisonPolicy::Panic)
    }

    /// Create a new [`SharedGlobalMut`], backed by a `Mutex` and optionally poisoning on panic.
    pub const fn new_mutex_with_policy(t: T, policy: PoisonPolicy) -> Self {
        Self {
            inner: SharedGlobalImpl::Value(SynchronizerSized::new(
                LockMetadata::poison_policy(policy),
                SynchronizerType::Mutex,
                t,
            )),
            projection: OnceLock::new(),
        }
    }

    /// Create a [`Shared`] reference, backed by this global. Note that the [`Shared`] cannot be unwrapped.
    pub const fn shared(&'static self) -> Shared<T> {
        Shared::from_inner(SharedImpl::ProjectionStaticRO(&self.inner))
    }

    /// Create a [`SharedMut`] reference, backed by this global. Note that the [`SharedMut`] cannot be unwrapped.
    pub const fn shared_mut(&'static self) -> SharedMut<T> {
        SharedMut::from_inner(SharedImpl::ProjectionStatic(&self.inner))
    }

    /// Lock the global [`SharedGlobalMut`] for reading directly.
    pub fn read(&'static self) -> SharedReadLock<'static, T> {
        self.inner.read()
    }

    /// Try to lock the global [`SharedGlobalMut`] for reading directly.
    pub fn try_read(&'static self) -> Option<SharedReadLock<'static, T>> {
        self.inner.try_lock_read()
    }

    /// Lock the global [`SharedGlobalMut`] for writing directly.
    pub fn write(&'static self) -> SharedWriteLock<'static, T> {
        self.inner.write()
    }

    /// Try to lock the global [`SharedGlobalMut`] for writing directly.
    pub fn try_write(&'static self) -> Option<SharedWriteLock<'static, T>> {
        self.inner.try_lock_write()
    }

    /// Cast this [`SharedGlobalMut`] to a new type.
    ///
    /// See [`Castable`] for more information.
    pub const fn cast<U>(&'static self) -> SharedMut<U>
    where
        T: Castable<U> + 'static,
        U: ?Sized + 'static,
    {
        // We want the vtable from SharedGlobalProjection, not SharedProjection
        let inner = std::ptr::from_ref(&self.inner).cast::<SharedGlobalProjection<T>>();
        SharedMut::from_inner(SharedImpl::ProjectionStatic(inner))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashMap;
    pub type Unsync = std::cell::Cell<()>;

    #[test]
    fn test_global() {
        static GLOBAL: SharedGlobalMut<usize> = SharedGlobalMut::new(1);
        static GLOBAL_UNSYNC: SharedGlobalMut<Unsync> =
            SharedGlobalMut::new_unsync(std::cell::Cell::new(()));

        let shared = GLOBAL.shared_mut();
        let shared2 = GLOBAL.shared();
        assert_eq!(shared.read(), 1);
        *shared.write() = 2;
        assert_eq!(shared.read(), 2);
        assert_eq!(shared2.read(), 2);

        let shared = GLOBAL_UNSYNC.shared_mut();
        assert_eq!(shared.read().get(), ());
        shared.write().set(());
        assert_eq!(shared.read().get(), ());
    }

    #[test]
    fn test_global_direct() {
        static GLOBAL: SharedGlobalMut<usize> = SharedGlobalMut::new(1);
        static GLOBAL_UNSYNC: SharedGlobalMut<Unsync> =
            SharedGlobalMut::new_unsync(std::cell::Cell::new(()));

        assert_eq!(GLOBAL.read(), 1);
        *GLOBAL.write() = 2;
        assert_eq!(GLOBAL.read(), 2);

        assert_eq!(GLOBAL_UNSYNC.read().get(), ());
        GLOBAL_UNSYNC.write().set(());
        assert_eq!(GLOBAL_UNSYNC.read().get(), ());
    }

    #[test]
    fn test_global_lazy() {
        static GLOBAL_LAZY: SharedGlobalMut<HashMap<&str, usize>> =
            SharedGlobalMut::new_lazy(|| HashMap::from_iter([("a", 1), ("b", 2)]));

        assert_eq!(GLOBAL_LAZY.read().len(), 2);

        let shared = GLOBAL_LAZY.shared_mut();
        assert_eq!(shared.read().len(), 2);
        shared.write().insert("c", 3);
        GLOBAL_LAZY.write().insert("d", 4);
        assert_eq!(shared.read().len(), 4);

        assert_eq!(GLOBAL_LAZY.read().len(), 4);
    }

    #[test]
    fn test_global_lazy_thread_race() {
        static GLOBAL: SharedGlobalMut<HashMap<&str, usize>> =
            SharedGlobalMut::new_lazy(|| HashMap::from_iter([("a", 1), ("b", 2)]));
        let shared = GLOBAL.shared();
        let mut handles = Vec::new();
        let barrier = Arc::new(std::sync::Barrier::new(100));
        for _ in 0..100 {
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let shared = GLOBAL.read();
                assert_eq!(shared.len(), 2);
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(shared.read().len(), 2);
    }

    #[test]
    fn test_global_cast() {
        static GLOBAL: SharedGlobalMut<[i32; 2]> = SharedGlobalMut::new_lazy(|| [1, 2]);
        let shared = GLOBAL.cast::<[i32]>();
        assert_eq!(shared.read().len(), 2);
        assert_eq!(GLOBAL.read().len(), 2);
        _ = shared.try_read().expect("Should be able to read");
        _ = shared.try_write().expect("Should be able to write");
    }
}
