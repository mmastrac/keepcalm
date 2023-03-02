#![doc = include_str!("../README.md")]

mod projection;
mod shared;
mod sharedrw;

pub use projection::{Projector, ProjectorRW};
pub use shared::Shared;
pub use sharedrw::{Implementation, PoisonPolicy, SharedRW};
