use std::{
    future::Future,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use crate::{erasedfuture::ErasedFuture, Shared};

trait BlockingFuture<B: BlockingFutureReturn<R>, R>: Future<Output = B> {}

trait BlockingFutureReturn<R> {
    fn map_return(self) -> R;
}

trait BlockingFutureFn<R: Send + 'static>: Send + 'static {}

/// We wrap our return types in this newtype to ensure that we can implement the traits we want without interference.
struct BlockingFutureReturnNewType<T: Send + 'static>(T);

impl<T: Send + 'static> From<T> for BlockingFutureReturnNewType<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T: Send + 'static> BlockingFutureReturn<T> for BlockingFutureReturnNewType<T> {
    fn map_return(self) -> T {
        self.0
    }
}

impl<T: Send + 'static, E> BlockingFutureReturn<T> for Result<BlockingFutureReturnNewType<T>, E> {
    fn map_return(self) -> T {
        match self {
            Ok(t) => t.0,
            Err(_) => panic!("Panic!"),
        }
    }
}

impl<T, R: BlockingFutureReturn<T>, F: Future<Output = R>> BlockingFuture<R, T> for F {}

impl<F: FnOnce() -> BlockingFutureReturnNewType<R> + Send + 'static, R: Send + 'static>
    BlockingFutureFn<R> for F
{
}

/// Generic, `async` spawn_blocking implementation that erases the underlying future type of the `spawn_blocking` method
/// that is passed to it. Note that this will probably only be useful when https://rust-lang.github.io/impl-trait-initiative/explainer/tait.html
/// stabilizes.
#[allow(unused)]
async fn spawn_blocking<
    T: Send + 'static,
    FN: FnOnce() -> BlockingFutureReturnNewType<T>,
    B: BlockingFutureReturn<T>,
    F: BlockingFuture<B, T>,
>(
    f: fn(FN) -> F,
    f2: FN,
) -> T {
    let res = f(f2).await;
    res.map_return()
}

#[allow(clippy::type_complexity)]
pub struct Spawner {
    spawner: fn(
        AtomicPtr<()>,
        AtomicPtr<()>,
        AtomicPtr<()>,
        fn(AtomicPtr<()>, AtomicPtr<()>, AtomicPtr<()>),
    ) -> ErasedFuture<()>,
}

impl Spawner {
    #[doc(hidden)]
    pub const fn __new(
        f: fn(
            AtomicPtr<()>,
            AtomicPtr<()>,
            AtomicPtr<()>,
            fn(AtomicPtr<()>, AtomicPtr<()>, AtomicPtr<()>),
        ) -> ErasedFuture<()>,
    ) -> Self {
        Self { spawner: f }
    }

    pub(crate) async fn spawn_blocking_map<A: Send, B: Send, F: FnOnce(&mut A) -> B>(
        &self,
        mut a: A,
        f: F,
    ) -> B {
        let input = AtomicPtr::new(&mut a as *mut A as *mut ());
        let mut output_storage = MaybeUninit::<B>::uninit();
        let output = AtomicPtr::new(output_storage.as_mut_ptr() as *mut ());
        let mut f = (AtomicBool::new(false), Some(f));
        let context = AtomicPtr::new((&mut f) as *mut (AtomicBool, Option<F>) as *mut ());
        (self.spawner)(context, input, output, |context, input, output| {
            let input = input.into_inner() as *mut A;
            let output = output.into_inner() as *mut B;

            // UNSAFETY:
            // A: We have a mut reference to this, and if we panic, the outer function will drop it
            // B: We have the MaybeUninit for B, and we set a flag if we've successfully initialized it
            // F: We own the function at this point and will drop it after we call it.
            unsafe {
                let f = (context.into_inner() as *mut (AtomicBool, Option<F>))
                    .as_mut()
                    .unwrap();
                std::ptr::write(output, (f.1.take().unwrap())(input.as_mut().unwrap()));
                f.0.store(true, Ordering::Release);
            }
        })
        .await;

        // UNSAFETY: If the atomic boolean is set to true, we have safely initialized the pointed inside the callback
        if f.0.load(Ordering::Acquire) {
            unsafe { output_storage.assume_init() }
        } else {
            panic!("Spawned operation panic!ed and did not complete");
        }
    }
}

// TODO: This is borrowed from the static_assertions crate, but should we just use it?
macro_rules! const_assert {
    ($x:expr $(,)?) => {
        #[allow(unknown_lints, clippy::eq_op)]
        const _: [(); 0 - !{
            const ASSERT: bool = $x;
            ASSERT
        } as usize] = [];
    };
}

const_assert!(std::mem::size_of::<Shared<()>>() == std::mem::size_of::<Shared<[usize; 1000]>>());
const_assert!(
    std::mem::size_of::<Shared<()>>() == std::mem::size_of::<Shared<&(dyn std::any::Any)>>()
);

/// Turns a `spawn_blocking` function like `tokio::task::spawn_blocking` `smol::unblock`, or
/// `async_std::task::spawn_blocking` into a type-erased object that can be used without knowledge of the runtime.
#[macro_export]
macro_rules! make_spawner {
    ($id:path) => {{
        use std::sync::atomic::AtomicPtr;
        let x = |context: AtomicPtr<()>,
                 input: AtomicPtr<()>,
                 output: AtomicPtr<()>,
                 f: fn(AtomicPtr<()>, AtomicPtr<()>, AtomicPtr<()>)| {
            $crate::_ErasedFuturePrivate::new_map(
                $id(move || {
                    f(context, input, output);
                }),
                drop,
            )
        };

        Spawner::__new(x)
    }};
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::Shared;
    use futures::FutureExt;

    #[allow(unused)]
    #[allow(unconditional_recursion)]
    fn ensure_blocking_future<R, B: BlockingFutureReturn<R>, F: BlockingFuture<B, R>>() {
        // Rename this type because it's so long
        type Empty = BlockingFutureReturnNewType<()>;
        ensure_blocking_future::<(), _, tokio::task::JoinHandle<Empty>>();
        ensure_blocking_future::<(), _, smol::Task<Empty>>();
        ensure_blocking_future::<(), _, async_std::task::JoinHandle<Empty>>();
    }

    #[allow(unused)]
    fn ensure_blocking_fn<R: Send + 'static, F: BlockingFutureFn<R>>(f: F) -> F {
        f
    }

    #[allow(unused)]
    fn ensure_blocking_fn_parent() {
        let f = || BlockingFutureReturnNewType::<()>(());
        let f = ensure_blocking_fn(f);
        let _ = tokio::task::spawn_blocking(f);
    }

    #[allow(unused)]
    async fn ensure_blocking_runners() {
        let x = spawn_blocking(tokio::task::spawn_blocking, || ().into()).await;
        let x = spawn_blocking(smol::unblock, || ().into()).await;
        let x = spawn_blocking(async_std::task::spawn_blocking, || ().into()).await;
    }

    #[allow(unused)]
    fn ensure_spawner_send_sync() {
        fn ensure_send<T: Sync>() {}
        fn ensure_sync<T: Sync>() {}
        ensure_send::<Spawner>();
        ensure_sync::<Spawner>();
    }

    #[tokio::test]
    async fn test_blocking() {
        let spawner = make_spawner!(tokio::task::spawn_blocking);
        let out = spawner.spawn_blocking_map(1, |input| *input + 1).await;
        assert_eq!(2, out);
    }

    #[tokio::test]
    async fn test_blocking_panic() {
        let spawner = make_spawner!(tokio::task::spawn_blocking);
        let out = spawner
            .spawn_blocking_map(1, |_| {
                panic!("Fail!");
                #[allow(unreachable_code)]
                {
                    1
                }
            })
            .catch_unwind()
            .await;
        assert!(out.is_err());
    }

    #[tokio::test]
    async fn test_blocking_with_lifetime() {
        let shared = Shared::new("123");
        let lock = spawn_blocking(tokio::task::spawn_blocking, move || {
            shared.read_owned().into()
        })
        .await;
        assert_eq!(lock, "123");
    }
}
