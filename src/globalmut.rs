use std::sync::{atomic::AtomicBool, Arc};

use once_cell::sync::OnceCell;
use parking_lot::{Mutex, RwLock};

use crate::{
    implementation::{SharedGlobalImpl, SharedImpl},
    PoisonPolicy, Shared, SharedMut,
};

/// A global version of [`SharedMut`]. Use [`SharedGlobalMut::shared`] to get a [`Shared`] to access the contents, or [`SharedGlobalMut::shared_mut`] to
/// get a [`SharedMut`].
pub struct SharedGlobalMut<T: Send> {
    inner: SharedGlobalImpl<T>,
}

impl<T: Send + Sync> SharedGlobalMut<T> {
    /// Create a new [`SharedGlobalMut`].
    pub const fn new(t: T) -> Self {
        Self {
            inner: SharedGlobalImpl::RwLock(
                PoisonPolicy::Panic,
                AtomicBool::new(false),
                RwLock::new(t),
            ),
        }
    }

    /// Create a new, lazy [`SharedGlobalMut`] implementation that will be initialized on the first access.
    pub const fn new_lazy(f: fn() -> T) -> Self {
        Self {
            inner: SharedGlobalImpl::RwLockLazy(
                PoisonPolicy::Panic,
                AtomicBool::new(false),
                OnceCell::new(),
                f,
            ),
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

    /// Create a new [`SharedGlobalMut`], backed by a `Mutex` and poisoning on panic.
    pub const fn new_mutex(t: T) -> Self {
        Self::new_mutex_with_policy(t, PoisonPolicy::Panic)
    }

    /// Create a new [`SharedGlobalMut`], backed by a `Mutex` and optionally poisoning on panic.
    pub const fn new_mutex_with_policy(t: T, policy: PoisonPolicy) -> Self {
        Self {
            inner: SharedGlobalImpl::Mutex(policy, AtomicBool::new(false), Mutex::new(t)),
        }
    }

    /// Create a [`Shared`] reference, backed by this global. Note that the [`Shared`] cannot be unwrapped.
    pub fn shared(&'static self) -> Shared<T> {
        // This is unnecessarily allocating an Arc each time, but it shouldn't be terribly expensive
        SharedImpl::ProjectionRO(Arc::new(&self.inner)).into()
    }

    /// Create a [`SharedMut`] reference, backed by this global. Note that the [`SharedMut`] cannot be unwrapped.
    pub fn shared_mut(&'static self) -> SharedMut<T> {
        // This is unnecessarily allocating an Arc each time, but it shouldn't be terribly expensive
        SharedImpl::Projection(Arc::new(&self.inner)).into()
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use super::*;

    pub type Unsync = std::cell::Cell<()>;

    static GLOBAL: SharedGlobalMut<usize> = SharedGlobalMut::new(1);
    static GLOBAL_UNSYNC: SharedGlobalMut<Unsync> =
        SharedGlobalMut::new_unsync(std::cell::Cell::new(()));
    static GLOBAL_LAZY: SharedGlobalMut<HashMap<&str, usize>> =
        SharedGlobalMut::new_lazy(|| HashMap::from_iter([("a", 1), ("b", 2)]));

    #[test]
    fn test_global() {
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
    fn test_global_lazy() {
        let shared = GLOBAL_LAZY.shared_mut();
        assert_eq!(shared.read().len(), 2);
        shared.write().insert("c", 3);
        assert_eq!(shared.read().len(), 3);
    }
}
