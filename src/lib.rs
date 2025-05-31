#![doc = include_str!("../README.md")]

#[cfg(feature = "async_experimental")]
mod asynchronous;
#[cfg(feature = "async_experimental")]
mod erasedfuture;
mod global;
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
pub use global::SharedGlobal;
pub use globalmut::SharedGlobalMut;
pub use implementation::PoisonPolicy;
pub use locks::{SharedReadLock, SharedWriteLock};
pub use projection::{Projector, ProjectorRW};
pub use shared::Shared;
pub use sharedmut::SharedMut;
