//! Experimental shared types that should make life easier.

mod projection;
mod shared;
mod sharedrw;

pub use projection::{Projector, ProjectorRW};
pub use shared::Shared;
pub use sharedrw::SharedRW;
