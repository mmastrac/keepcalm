use core::panic;
use std::{future::Future, mem::MaybeUninit, sync::atomic::AtomicPtr};

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

pub struct Spawner {
    spawner: Box<
        dyn Fn(
            AtomicPtr<()>,
            AtomicPtr<()>,
            AtomicPtr<()>,
            fn(AtomicPtr<()>, AtomicPtr<()>, AtomicPtr<()>),
        ) -> ErasedFuture<()>,
    >,
}

impl Spawner {
    pub(crate) async fn spawn_blocking_map<A, B, F: FnOnce(&mut A) -> B>(
        &self,
        mut a: A,
        mut f: F,
    ) -> B {
        let input = AtomicPtr::new(&mut a as *mut A as *mut ());
        let mut output_storage = MaybeUninit::<B>::uninit();
        let output = AtomicPtr::new(output_storage.as_mut_ptr() as *mut ());
        let mut f = Some(f);
        let context = AtomicPtr::new(&mut f as *mut Option<F> as *mut ());
        (self.spawner)(context, input, output, |context, input, output| {
            let input = input.into_inner() as *mut A;
            let output = output.into_inner() as *mut B;
            let f = context.into_inner() as *mut Option<F>;

            unsafe {
                std::ptr::write(
                    output,
                    (f.as_mut().unwrap().take().unwrap())(input.as_mut().unwrap()),
                );
            }
        })
        .await;
        unsafe { output_storage.assume_init() }
    }
}

macro_rules! const_assert {
    ($x:expr $(,)?) => {
        #[allow(unknown_lints, eq_op)]
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

        Spawner {
            spawner: Box::new(x),
        }
    }};
}

#[cfg(test)]
mod test {
    use crate::Shared;

    use super::*;

    fn ensure_blocking_future<R, B: BlockingFutureReturn<R>, F: BlockingFuture<B, R>>() {
        // Rename this type because it's so long
        type Empty = BlockingFutureReturnNewType<()>;
        ensure_blocking_future::<(), _, tokio::task::JoinHandle<Empty>>();
        ensure_blocking_future::<(), _, smol::Task<Empty>>();
        ensure_blocking_future::<(), _, async_std::task::JoinHandle<Empty>>();
    }

    fn ensure_blocking_fn<R: Send + 'static, F: BlockingFutureFn<R>>(f: F) -> F {
        f
    }

    fn ensure_blocking_fn_parent() {
        let f = || BlockingFutureReturnNewType::<()>(());
        let f = ensure_blocking_fn(f);
        let ret = tokio::task::spawn_blocking(f);
    }

    async fn ensure_blocking_runners() {
        let x = spawn_blocking(tokio::task::spawn_blocking, || ().into()).await;
        let x = spawn_blocking(smol::unblock, || ().into()).await;
        let x = spawn_blocking(async_std::task::spawn_blocking, || ().into()).await;
    }

    #[tokio::test]
    async fn test() {
        let t = ();

        let spawner = make_spawner!(tokio::task::spawn_blocking);
        let out = spawner.spawn_blocking_map(1, |input| *input + 1).await;
        assert_eq!(2, out);
        // let out = (spawner.spawner)([0; BUF_SIZE], |input| {
        //     input
        // }).await;
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
