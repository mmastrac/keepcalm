use crate::{
    implementation::{
        LockMetadata, SharedGlobalImpl, SharedGlobalProjection, SharedImpl, SharedProjection,
    },
    synchronizer::{SynchronizerSized, SynchronizerType},
    Castable, PoisonPolicy, Shared, SharedReadLock,
};

/// A global version of [`Shared`]. Use [`SharedGlobal::shared`] to get a [`Shared`] to access the contents.
pub struct SharedGlobal<T> {
    inner: SharedGlobalImpl<T>,
}

impl<T: Send + Sync> SharedGlobal<T> {
    /// Create a new [`SharedGlobal`].
    pub const fn new(t: T) -> Self {
        Self {
            inner: SharedGlobalImpl::Value(SynchronizerSized::new(
                LockMetadata::poison_ignore(),
                SynchronizerType::Arc,
                t,
            )),
        }
    }

    /// Create a new, lazy [`SharedGlobal`] implementation that will be initialized on the first access.
    pub const fn new_lazy(f: fn() -> T) -> Self {
        Self {
            inner: SharedGlobalImpl::lazy(f, SynchronizerType::Arc, PoisonPolicy::Ignore),
        }
    }
}

impl<T: Send> SharedGlobal<T> {
    /// The [`SharedGlobal::new`] function requires a type that is both `Send + Sync`. If this is not possible, you may call
    /// [`SharedGlobal::new_unsync`] to create a [`SharedGlobal`] implementation that uses a [`Mutex`].
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
    /// SharedGlobal::new(Unsync {});
    /// ```
    ///
    /// This will work:
    ///
    /// ```rust
    /// # use std::{cell::Cell, marker::PhantomData};
    /// # use keepcalm::*;
    /// # pub type Unsync = PhantomData<Cell<()>>;
    /// SharedGlobal::new_unsync(Unsync {});
    /// ```
    pub const fn new_unsync(t: T) -> Self {
        Self::new_mutex(t)
    }

    /// The [`SharedGlobal::new_lazy`] function requires a type that is both `Send + Sync`. If this is not possible, you may call
    /// [`SharedGlobal::new_lazy`] to create a [`SharedGlobal`] implementation that uses a [`Mutex`].
    pub const fn new_lazy_unsync(f: fn() -> T) -> Self {
        Self {
            inner: SharedGlobalImpl::lazy(f, SynchronizerType::Mutex, PoisonPolicy::Panic),
        }
    }

    /// Create a new [`SharedGlobal`], backed by a `Mutex` and poisoning on panic.
    pub const fn new_mutex(t: T) -> Self {
        Self::new_mutex_with_policy(t, PoisonPolicy::Panic)
    }

    /// Create a new [`SharedGlobal`], backed by a `Mutex` and optionally poisoning on panic.
    pub const fn new_mutex_with_policy(t: T, policy: PoisonPolicy) -> Self {
        Self {
            inner: SharedGlobalImpl::Value(SynchronizerSized::new(
                LockMetadata::poison_policy(policy),
                SynchronizerType::Mutex,
                t,
            )),
        }
    }

    /// Create a [`Shared`] reference, backed by this global. Note that the [`Shared`] cannot be unwrapped.
    pub const fn shared(&'static self) -> Shared<T> {
        let inner: &'static dyn SharedProjection<T> = &self.inner as _;
        Shared::from_inner(SharedImpl::ProjectionStaticRO(inner))
    }

    /// Lock the global [`SharedGlobal`] for reading directly.
    pub fn read(&'static self) -> SharedReadLock<'static, T> {
        self.inner.read()
    }

    /// Cast this [`SharedGlobal`] to a new type.
    ///
    /// See [`Castable`] for more information.
    pub const fn cast<U>(&'static self) -> Shared<U>
    where
        T: Castable<U> + 'static,
        U: ?Sized + 'static,
    {
        // We want the vtable from SharedGlobalProjection, not SharedProjection
        let inner = std::ptr::from_ref(&self.inner).cast::<SharedGlobalProjection<T>>();
        Shared::from_inner(SharedImpl::ProjectionStaticRO(inner))
    }
}

#[cfg(test)]
mod test {
    use super::SharedGlobal;
    use std::{collections::HashMap, sync::Arc};
    pub type Unsync = std::cell::Cell<()>;

    static GLOBAL: SharedGlobal<usize> = SharedGlobal::new(1);
    static GLOBAL_UNSYNC: SharedGlobal<Unsync> = SharedGlobal::new_unsync(std::cell::Cell::new(()));
    static GLOBAL_LAZY: SharedGlobal<HashMap<&str, usize>> =
        SharedGlobal::new_lazy(|| HashMap::from_iter([("a", 1), ("b", 2)]));

    #[test]
    fn test_global() {
        let shared = GLOBAL.shared();
        assert_eq!(shared.read(), 1);
        let shared = GLOBAL_UNSYNC.shared();
        assert_eq!(shared.read().get(), ());
    }

    #[test]
    fn test_global_direct() {
        assert_eq!(GLOBAL.read(), 1);
        assert_eq!(GLOBAL_UNSYNC.read().get(), ());
    }

    #[test]
    fn test_global_lazy() {
        let shared = GLOBAL_LAZY.shared();
        assert_eq!(shared.read().len(), 2);
    }

    #[test]
    fn test_global_lazy_thread_race() {
        static GLOBAL: SharedGlobal<HashMap<&str, usize>> =
            SharedGlobal::new_lazy(|| HashMap::from_iter([("a", 1), ("b", 2)]));
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
    fn test_global_lazy_direct() {
        assert_eq!(GLOBAL_LAZY.read().len(), 2);
    }
}
