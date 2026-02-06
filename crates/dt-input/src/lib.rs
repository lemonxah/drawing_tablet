//! Virtual tablet input injection using uinput.
//!
//! This crate creates a virtual tablet device that can inject stylus
//! and multitouch events into the Linux input subsystem.

mod tablet;
mod events;

pub use tablet::VirtualTablet;
pub use events::*;
