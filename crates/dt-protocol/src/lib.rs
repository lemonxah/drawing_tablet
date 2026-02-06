//! Protocol definitions for drawing tablet streaming.
//!
//! This crate defines the packet types used for communication between
//! the Linux server and Android client.

mod packets;
mod codec;

pub use packets::*;
pub use codec::*;
