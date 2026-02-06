//! Network packet definitions for the drawing tablet protocol.

use serde::{Deserialize, Serialize};

/// Protocol version for compatibility checking.
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum framed packet size (constrained by the u16 length field).
pub const MAX_PACKET_SIZE: usize = 65535;

/// Maximum H.264 payload bytes per UDP fragment.  Keeps each datagram small
/// enough to avoid IP-level fragmentation on typical LAN MTUs (Ethernet 1500 - headers).
pub const MAX_FRAGMENT_DATA: usize = 1400;

/// Packet type identifiers.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    Video = 0x01,
    Input = 0x02,
    Control = 0x03,
}

impl TryFrom<u8> for PacketType {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(PacketType::Video),
            0x02 => Ok(PacketType::Input),
            0x03 => Ok(PacketType::Control),
            other => Err(other),
        }
    }
}

/// Video packet sent from server to client.
///
/// Large encoded frames are split into multiple packets that share the same
/// `sequence` number.  The receiver reassembles them by concatenating
/// `fragment_index` 0 .. `fragment_count-1` in order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoPacket {
    /// Sequence number for ordering and loss detection.
    pub sequence: u32,
    /// Timestamp in microseconds.
    pub timestamp: u64,
    /// Whether this frame is a keyframe (I-frame).
    pub is_keyframe: bool,
    /// Zero-based index of this fragment within the frame.
    pub fragment_index: u16,
    /// Total number of fragments for this frame (1 = unfragmented).
    pub fragment_count: u16,
    /// H.264 NAL unit data (a slice of the full frame when fragmented).
    pub data: Vec<u8>,
}

/// Input packet sent from client to server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputPacket {
    /// Timestamp in microseconds.
    pub timestamp: u64,
    /// The input event.
    pub event: InputEvent,
}

/// Input events from the tablet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum InputEvent {
    /// Stylus moved (while hovering or touching).
    StylusMove {
        /// X position normalized to 0.0-1.0.
        x: f32,
        /// Y position normalized to 0.0-1.0.
        y: f32,
        /// Pressure normalized to 0.0-1.0.
        pressure: f32,
        /// X tilt in degrees (-90 to 90).
        tilt_x: f32,
        /// Y tilt in degrees (-90 to 90).
        tilt_y: f32,
    },
    /// Stylus touched the screen.
    StylusDown {
        x: f32,
        y: f32,
        pressure: f32,
        tilt_x: f32,
        tilt_y: f32,
    },
    /// Stylus lifted from the screen.
    StylusUp,
    /// Stylus button pressed or released.
    StylusButton {
        /// Button index (0 = primary, 1 = secondary).
        button: u8,
        /// Whether the button is pressed.
        pressed: bool,
    },
    /// Touch finger down.
    TouchDown {
        /// Touch tracking ID.
        id: u32,
        /// X position normalized to 0.0-1.0.
        x: f32,
        /// Y position normalized to 0.0-1.0.
        y: f32,
    },
    /// Touch finger moved.
    TouchMove { id: u32, x: f32, y: f32 },
    /// Touch finger lifted.
    TouchUp { id: u32 },
}

/// Control packets for connection management.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlPacket {
    /// Client requests connection.
    Connect {
        /// Protocol version.
        version: u32,
    },
    /// Server acknowledges connection.
    ConnectAck {
        /// Stream width in pixels.
        width: u32,
        /// Stream height in pixels.
        height: u32,
    },
    /// Connection refused with reason.
    ConnectNack { reason: String },
    /// Keep-alive heartbeat.
    Heartbeat,
    /// Client requests a keyframe.
    RequestKeyframe,
    /// Graceful disconnect.
    Disconnect,
}

/// A framed packet ready for transmission.
#[derive(Debug, Clone)]
pub struct FramedPacket {
    pub packet_type: PacketType,
    pub payload: Vec<u8>,
}
