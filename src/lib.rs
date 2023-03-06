#![doc = include_str!("../README.md")]

#[cfg(feature = "global_experimental")]
mod global;
#[cfg(feature = "global_experimental")]
mod globalmut;
mod implementation;
mod locks;
mod projection;
mod shared;
mod sharedmut;

#[cfg(feature = "global_experimental")]
pub use global::SharedGlobal;
#[cfg(feature = "global_experimental")]
pub use globalmut::SharedGlobalMut;
pub use implementation::PoisonPolicy;
pub use locks::{SharedReadLock, SharedWriteLock};
pub use projection::{Projector, ProjectorRW};
pub use shared::Shared;
pub use sharedmut::SharedMut;
