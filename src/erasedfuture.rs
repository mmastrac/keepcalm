use std::{mem::MaybeUninit, ops::DerefMut, pin::Pin};

use futures::Future;

const FUTURE_BUF_SIZE: usize = 32;

/// Erases a future of up to 32 bytes, with an alignment less than or equal to 8 bytes.
#[repr(align(8))]
pub struct ErasedFuture<T: 'static> {
    buffer: [u8; FUTURE_BUF_SIZE],
    map_fn: *const fn(),
    poll_fn: fn(&mut ErasedFuture<T>, &mut std::task::Context<'_>) -> std::task::Poll<T>,
    pointee_fn: fn(*mut [u8; FUTURE_BUF_SIZE]) -> *mut (dyn Future<Output = T> + Unpin),
}

trait ErasedFutureNew<F: Future<Output = T> + Unpin + 'static, T> {
    const SIZE_OK: ();
    const ALIGN_OK: ();
    fn new(f: F) -> ErasedFuture<T>;
}

impl<T> ErasedFuture<T> {
    pub fn new<F: Future<Output = T> + Unpin + 'static>(f: F) -> Self {
        <Self as ErasedFutureNew<F, T>>::new(f)
    }

    fn get_ptr(&mut self) -> *mut (dyn Future<Output = T> + Unpin) {
        (self.pointee_fn)(&mut self.buffer)
    }
}

impl<F: Future<Output = T> + Unpin + 'static, T> ErasedFutureNew<F, T> for ErasedFuture<T> {
    const SIZE_OK: () = assert!(std::mem::size_of::<F>() <= FUTURE_BUF_SIZE);
    const ALIGN_OK: () =
        assert!(std::mem::align_of::<F>() <= std::mem::align_of::<ErasedFuture<T>>());

    fn new(f: F) -> Self {
        let _ = <Self as ErasedFutureNew<F, T>>::SIZE_OK;
        let _ = <Self as ErasedFutureNew<F, T>>::ALIGN_OK;

        // Re-check the assertions at runtime
        assert!(std::mem::size_of::<F>() <= FUTURE_BUF_SIZE);
        assert!(std::mem::align_of::<F>() <= std::mem::align_of::<ErasedFuture<T>>());

        // Initialize our erased future
        let mut erased_future = MaybeUninit::<ErasedFuture<T>>::uninit();
        let init_ptr = erased_future.as_mut_ptr();

        unsafe {
            // Move f to the buffer and forget about it - we will drop it later
            let bufptr = (*init_ptr).buffer.as_mut_ptr() as *mut F;
            std::ptr::copy_nonoverlapping(&f, bufptr, 1);
            std::mem::forget(f);

            // Zero out the end of the buffer to avoid uninitialized bytes
            (*init_ptr).buffer[std::mem::size_of::<F>()..FUTURE_BUF_SIZE].fill(0);

            // Create a function that makes fat pointers from thin ones, with knowledge of the original type F
            (*init_ptr).pointee_fn = |ptr| ptr as *mut F as *mut (dyn Future<Output = T> + Unpin);

            (*init_ptr).poll_fn = |this: &mut ErasedFuture<T>, cx| {
                let f = this.get_ptr();
                Pin::new(f.as_mut().unwrap()).poll(cx)
            };

            erased_future.assume_init()
        }
    }
}

impl<T> Drop for ErasedFuture<T> {
    fn drop(&mut self) {
        unsafe { std::ptr::drop_in_place(self.get_ptr()) }
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

    struct MyFuture {
        x: [u8; 1],
    }

    impl Future for MyFuture {
        type Output = usize;
        fn poll(
            self: Pin<&mut Self>,
            cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Self::Output> {
            std::task::Poll::Ready(1)
        }
    }

    impl Drop for MyFuture {
        fn drop(&mut self) {}
    }

    #[tokio::test]
    async fn test_erase_future() {
        let outside = [1; 1];
        ErasedFuture::new(MyFuture { x: outside }).await;
        let outside = [1_usize; 100];
        println!(
            "{:?}",
            ErasedFuture::new(tokio::task::spawn_blocking(move || outside)).await
        );
    }
}
