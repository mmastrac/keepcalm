#![doc = include_str!("../README.md")]

mod implementation;
mod locks;
mod projection;
mod shared;
mod sharedmut;

pub use implementation::PoisonPolicy;
pub use locks::{SharedReadLock, SharedWriteLock};
pub use projection::{Projector, ProjectorRW};
pub use shared::Shared;
pub use sharedmut::SharedMut;
