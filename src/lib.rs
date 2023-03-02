#![doc = include_str!("../README.md")]

mod implementation;
mod locks;
mod projection;
mod shared;
mod sharedrw;

pub use projection::{Projector, ProjectorRW};
pub use shared::Shared;
pub use sharedrw::{Implementation, SharedRW};
pub use implementation::PoisonPolicy;
