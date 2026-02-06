//! Video encoding using GStreamer with VA-API hardware acceleration.
//!
//! This crate provides H.264 video encoding with low-latency settings
//! suitable for real-time screen streaming.

mod vaapi;
mod software;

pub use vaapi::{VideoEncoder, EncoderConfig, EncodedFrame, EncoderError};
