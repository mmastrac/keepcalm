use std::sync::{atomic::AtomicBool, Arc};

use once_cell::sync::OnceCell;
use parking_lot::Mutex;

use crate::{
    implementation::{SharedGlobalImpl, SharedImpl},
    PoisonPolicy, Shared,
};

/// A global version of [`Shared`]. Use [`SharedGlobal::shared`] to get a [`Shared`] to access the contents.
pub struct SharedGlobal<T: Send> {
    inner: SharedGlobalImpl<T>,
}

impl<T: Send + Sync> SharedGlobal<T> {
    /// Create a new [`SharedGlobal`].
    pub const fn new(t: T) -> Self {
        Self {
            inner: SharedGlobalImpl::Raw(t),
        }
    }

    /// Create a new, lazy [`SharedGlobal`] implementation that will be initialized on the first access.
    pub const fn new_lazy(f: fn() -> T) -> Self {
        Self {
            inner: SharedGlobalImpl::RawLazy(OnceCell::new(), f),
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
            inner: SharedGlobalImpl::MutexLazy(
                PoisonPolicy::Panic,
                AtomicBool::new(false),
                OnceCell::new(),
                f,
            ),
        }
    }

    /// Create a new [`SharedGlobal`], backed by a `Mutex` and poisoning on panic.
    pub const fn new_mutex(t: T) -> Self {
        Self::new_mutex_with_policy(t, PoisonPolicy::Panic)
    }

    /// Create a new [`SharedGlobal`], backed by a `Mutex` and optionally poisoning on panic.
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
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use super::SharedGlobal;

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
    fn test_global_lazy() {
        let shared = GLOBAL_LAZY.shared();
        assert_eq!(shared.read().len(), 2);
    }
}
