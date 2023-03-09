use core::panic;
use std::{future::Future, marker::PhantomData};

trait BlockingFuture<B: BlockingFutureReturn<R>, R>: Future<Output = B> {}

trait BlockingFutureReturn<R> {
    fn map(self) -> R;
}

trait BlockingFutureFn<R: Send + 'static>: Send + 'static {}

/// We wrap our return types in this newtype to ensure that we can implement the traits we want without interference.
struct BlockingFutureReturnNewType<T: Send + 'static>(T);

impl <T: Send + 'static> From<T> for BlockingFutureReturnNewType<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T: Send + 'static> BlockingFutureReturn<T> for BlockingFutureReturnNewType<T> {
    fn map(self) -> T {
        self.0
    }
}

impl<T: Send + 'static, E> BlockingFutureReturn<T> for Result<BlockingFutureReturnNewType<T>, E> {
    fn map(self) -> T {
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

/// Generic spawn_blocking implementation
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
    res.map()
}

#[cfg(test)]
mod test {
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

    fn ensure_blocking_runners() {
        let _ = spawn_blocking(tokio::task::spawn_blocking, || ().into());
        let _ = spawn_blocking(smol::unblock, || ().into());
        let _ = spawn_blocking(async_std::task::spawn_blocking, || ().into());
    }
}
