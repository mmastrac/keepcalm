use std::future::Future;
use std::{mem::MaybeUninit, ops::DerefMut, pin::Pin};

const FUTURE_BUF_SIZE: usize = 32;

/// Erases a future of up to 32 bytes, with an alignment less than or equal to 8 bytes.
#[repr(align(8))]
pub struct ErasedFuture<T: 'static> {
    /// The buffer in which we store the representation of the future. The underlying future must be [`Unpin`] as
    /// [`ErasedFuture`] is marked [`Unpin`].
    buffer: [u8; FUTURE_BUF_SIZE],

    /// A mapping function, never null.
    map_fn: *const fn(),

    /// A polling function that knows the underlying type of the future.
    poll_fn: fn(&mut ErasedFuture<T>, &mut std::task::Context<'_>) -> std::task::Poll<T>,

    /// A drop function.
    drop_fn: fn(&mut ErasedFuture<T>),
}

trait ErasedFutureNew<F: Future<Output = T2> + Unpin + 'static, T2, T> {
    const SIZE_OK: ();
    const ALIGN_OK: ();
    fn new_mapped(f: F, map: fn(T2) -> T) -> Self;
}

impl<T> ErasedFuture<T> {
    pub fn new<F: Future<Output = T> + Send + Unpin + 'static>(f: F) -> Self {
        <Self as ErasedFutureNew<F, T, T>>::new_mapped(f, std::convert::identity)
    }

    pub fn new_map<T2, F: Future<Output = T2> + Send + Unpin + 'static>(
        f: F,
        map: fn(T2) -> T,
    ) -> Self {
        <Self as ErasedFutureNew<F, T2, T>>::new_mapped(f, map)
    }
}

/// UNSAFETY: We require the underlying future to be Send
unsafe impl<T> Send for ErasedFuture<T> {}

impl<F: Future<Output = T2> + Unpin + 'static, T2, T> ErasedFutureNew<F, T2, T>
    for ErasedFuture<T>
{
    const SIZE_OK: () = assert!(std::mem::size_of::<F>() <= FUTURE_BUF_SIZE);
    const ALIGN_OK: () =
        assert!(std::mem::align_of::<F>() <= std::mem::align_of::<ErasedFuture<T>>());

    fn new_mapped(f: F, map: fn(T2) -> T) -> Self {
        #[allow(clippy::let_unit_value)]
        let _ = <Self as ErasedFutureNew<F, T2, T>>::SIZE_OK;
        #[allow(clippy::let_unit_value)]
        let _ = <Self as ErasedFutureNew<F, T2, T>>::ALIGN_OK;

        // Re-check the assertions at runtime
        assert!(std::mem::size_of::<F>() <= FUTURE_BUF_SIZE);
        assert!(std::mem::align_of::<F>() <= std::mem::align_of::<ErasedFuture<T>>());

        // Initialize our erased future
        let mut erased_future = MaybeUninit::<ErasedFuture<T>>::uninit();
        let init_ptr = erased_future.as_mut_ptr();

        // UNSAFETY: We assume this block cannot panic
        unsafe {
            // Move f to the buffer and forget about it - we will drop it later
            let bufptr = (*init_ptr).buffer.as_mut_ptr() as *mut F;
            std::ptr::copy_nonoverlapping(&f, bufptr, 1);
            std::mem::forget(f);

            // Zero out the end of the buffer to avoid uninitialized bytes
            (*init_ptr).buffer[std::mem::size_of::<F>()..FUTURE_BUF_SIZE].fill(0);

            // Store the map function as a boring () -> () function pointer to erase the type
            (*init_ptr).map_fn = map as *const fn();

            // Create a function that makes fat pointers from thin ones, with knowledge of the original type F
            (*init_ptr).drop_fn = |this| {
                let ptr = this.buffer.as_mut_ptr() as *mut F;
                std::ptr::drop_in_place(ptr)
            };

            (*init_ptr).poll_fn = |this, cx| {
                let f = this.buffer.as_mut_ptr() as *mut F;
                let map: fn(T2) -> T = std::mem::transmute(this.map_fn);
                Pin::new(f.as_mut().unwrap()).poll(cx).map(map)
            };

            // UNSAFETY: All fields initialized at this point
            erased_future.assume_init()
        }
    }
}

impl<T> Drop for ErasedFuture<T> {
    fn drop(&mut self) {
        (self.drop_fn)(self)
    }
}

impl<T> Future for ErasedFuture<T> {
    type Output = T;

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        (self.deref_mut().poll_fn)(self.deref_mut(), cx)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::FutureExt;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct MyFuture<T: Unpin> {
        x: Option<T>,
    }

    impl<T: Unpin> Future for MyFuture<T> {
        type Output = T;
        fn poll(
            mut self: Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            std::task::Poll::Ready(self.deref_mut().x.take().unwrap())
        }
    }

    impl<T: Unpin> Drop for MyFuture<T> {
        fn drop(&mut self) {}
    }

    /// Ensure that the erased future is dropped once and only once, even if the future panics.
    #[tokio::test]
    async fn test_erased_future_drops() {
        static DROPPED: AtomicBool = AtomicBool::new(false);
        struct DroppableFuture {
            panic: bool,
        }
        impl Future for DroppableFuture {
            type Output = ();
            fn poll(
                self: Pin<&mut Self>,
                _cx: &mut std::task::Context<'_>,
            ) -> std::task::Poll<Self::Output> {
                if self.panic {
                    panic!("Expected! Panic! at the poll!");
                }
                std::task::Poll::Ready(())
            }
        }
        impl Drop for DroppableFuture {
            fn drop(&mut self) {
                if DROPPED.load(Ordering::SeqCst) {
                    panic!("**UNEXPECTED** Dropped more than once! (this is bad)");
                }
                DROPPED.store(true, Ordering::SeqCst);
            }
        }

        assert!(!DROPPED.load(Ordering::SeqCst));
        ErasedFuture::new(DroppableFuture { panic: false }).await;
        assert!(DROPPED.load(Ordering::SeqCst));
        DROPPED.store(false, Ordering::SeqCst);

        assert!(!DROPPED.load(Ordering::SeqCst));
        let res = ErasedFuture::new(DroppableFuture { panic: true })
            .catch_unwind()
            .await;
        assert!(res.is_err());
        assert!(DROPPED.load(Ordering::SeqCst));
    }

    /// Erase a small future.
    #[tokio::test]
    async fn test_erase_future() {
        assert_eq!(
            ErasedFuture::new(MyFuture {
                x: Some([1_usize; 1])
            })
            .await,
            [1_usize; 1]
        );
    }

    /// Erase a large future.
    #[tokio::test]
    async fn test_erase_large_future() {
        let outside = [1_usize; 100];
        _ = ErasedFuture::new(tokio::task::spawn_blocking(move || outside)).await;
    }
}
