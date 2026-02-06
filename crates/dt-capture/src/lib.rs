//! Screen capture using PipeWire and XDG Desktop Portal.
//!
//! This crate provides screen capture functionality using the PipeWire
//! multimedia framework and the XDG Desktop Portal for screen selection.

mod portal;
mod pipewire;

pub use portal::{MonitorInfo, select_monitor};
pub use pipewire::{Frame, FrameData, PixelFormat, ScreenCapture, CaptureError};
