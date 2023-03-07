#[macro_export]
macro_rules! async_read {
    ($lock:ident, $($fn:ident)::+) => {
        async {
            trait GetTFrom<T> {
                fn get(self) -> T;
            }

            impl <T> GetTFrom<T> for T {
                fn get(self) -> T {
                    self
                }
            }

            impl <T, E: std::error::Error> GetTFrom<T> for Result<T, E> {
                fn get(self) -> T {
                    self.expect("Failed to lock asynchronously")
                }
            }

            let lock: $crate::SharedReadLock<_> = $($fn)::*(|| $lock.read()).await.get();
            lock
        }
    };
}

#[macro_export]
macro_rules! async_write {
    ($lock:ident, $($fn:ident)::+) => {
        async {
            trait GetTFrom<T> {
                fn get(self) -> T;
            }

            impl <T> GetTFrom<T> for T {
                fn get(self) -> T {
                    self
                }
            }

            impl <T, E: std::error::Error> GetTFrom<T> for Result<T, E> {
                fn get(self) -> T {
                    self.expect("Failed to lock asynchronously")
                }
            }

            let lock: $crate::SharedWriteLock<_> = $($fn)::*(|| $lock.write()).await.get();
            lock
        }
    };
}

#[cfg(test)]
mod test {
    use crate::SharedGlobalMut;

    #[tokio::test]
    async fn test_async_lock_tokio() {
        static GLOBAL: SharedGlobalMut<usize> = SharedGlobalMut::new(1);
        assert_eq!(async_read!(GLOBAL, tokio::task::spawn_blocking).await, 1);
        *async_write!(GLOBAL, tokio::task::spawn_blocking).await = 2;
        assert_eq!(async_write!(GLOBAL, tokio::task::spawn_blocking).await, 2);
    }

    #[test]
    fn test_async_lock_smol() {
        static GLOBAL: SharedGlobalMut<usize> = SharedGlobalMut::new(1);
        smol::block_on(async {
            assert_eq!(async_read!(GLOBAL, smol::unblock).await, 1);
            *async_write!(GLOBAL, smol::unblock).await = 2;
            assert_eq!(async_write!(GLOBAL, smol::unblock).await, 2);
        });
    }
}
