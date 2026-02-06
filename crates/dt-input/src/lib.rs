//! Virtual tablet input injection using uinput.
//!
//! This crate creates a virtual tablet device that can inject stylus
//! and multitouch events into the Linux input subsystem.

mod events;
mod gesture;
mod retimer;
mod tablet;

pub use events::*;
pub use gesture::*;
pub use retimer::EventRetimer;
pub use tablet::{TabletDescriptor, VirtualTablet};
