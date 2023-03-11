#![doc = include_str!("../README.md")]

#[cfg(feature = "async_experimental")]
mod asynchronous;
#[cfg(feature = "async_experimental")]
mod erasedfuture;
#[cfg(feature = "global_experimental")]
mod global;
#[cfg(feature = "global_experimental")]
mod globalmut;
mod implementation;
mod locks;
mod projection;
mod rcu;
mod shared;
mod sharedmut;
mod synchronizer;

#[cfg(feature = "async_experimental")]
pub use asynchronous::Spawner;
#[cfg(feature = "async_experimental")]
pub use erasedfuture::ErasedFuture as _ErasedFuturePrivate;
#[cfg(feature = "global_experimental")]
pub use global::SharedGlobal;
#[cfg(feature = "global_experimental")]
pub use globalmut::SharedGlobalMut;
pub use implementation::PoisonPolicy;
pub use locks::{SharedReadLock, SharedWriteLock};
pub use projection::{Projector, ProjectorRW};
pub use shared::Shared;
pub use sharedmut::SharedMut;
