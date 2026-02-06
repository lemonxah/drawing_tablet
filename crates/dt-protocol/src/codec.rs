//! Serialization and framing for network packets.

use crate::packets::*;
use thiserror::Error;

/// Errors that can occur during packet encoding/decoding.
#[derive(Debug, Error)]
pub enum CodecError {
    #[error("packet too large: {size} bytes (max {max})")]
    PacketTooLarge { size: usize, max: usize },

    #[error("packet too small: need at least 3 bytes for header")]
    PacketTooSmall,

    #[error("unknown packet type: {0}")]
    UnknownPacketType(u8),

    #[error("serialization error: {0}")]
    SerializationError(#[from] bincode::Error),

    #[error("incomplete packet: expected {expected} bytes, got {actual}")]
    IncompletePacket { expected: usize, actual: usize },
}

/// Encode a video packet into a framed packet.
pub fn encode_video(packet: &VideoPacket) -> Result<Vec<u8>, CodecError> {
    let payload = bincode::serialize(packet)?;
    encode_framed(PacketType::Video, &payload)
}

/// Encode an input packet into a framed packet.
pub fn encode_input(packet: &InputPacket) -> Result<Vec<u8>, CodecError> {
    let payload = bincode::serialize(packet)?;
    encode_framed(PacketType::Input, &payload)
}

/// Encode a control packet into a framed packet.
pub fn encode_control(packet: &ControlPacket) -> Result<Vec<u8>, CodecError> {
    let payload = bincode::serialize(packet)?;
    encode_framed(PacketType::Control, &payload)
}

/// Encode a framed packet with header: [type: u8][length: u16][payload].
fn encode_framed(packet_type: PacketType, payload: &[u8]) -> Result<Vec<u8>, CodecError> {
    if payload.len() > MAX_PACKET_SIZE - 3 {
        return Err(CodecError::PacketTooLarge {
            size: payload.len() + 3,
            max: MAX_PACKET_SIZE,
        });
    }

    let len = payload.len() as u16;
    let mut buf = Vec::with_capacity(3 + payload.len());
    buf.push(packet_type as u8);
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(payload);
    Ok(buf)
}

/// Decoded packet variants.
#[derive(Debug, Clone)]
pub enum DecodedPacket {
    Video(VideoPacket),
    Input(InputPacket),
    Control(ControlPacket),
}

/// Decode a framed packet from raw bytes.
pub fn decode_packet(data: &[u8]) -> Result<DecodedPacket, CodecError> {
    if data.len() < 3 {
        return Err(CodecError::PacketTooSmall);
    }

    let packet_type = PacketType::try_from(data[0])
        .map_err(CodecError::UnknownPacketType)?;
    let len = u16::from_le_bytes([data[1], data[2]]) as usize;

    if data.len() < 3 + len {
        return Err(CodecError::IncompletePacket {
            expected: 3 + len,
            actual: data.len(),
        });
    }

    let payload = &data[3..3 + len];

    match packet_type {
        PacketType::Video => {
            let packet: VideoPacket = bincode::deserialize(payload)?;
            Ok(DecodedPacket::Video(packet))
        }
        PacketType::Input => {
            let packet: InputPacket = bincode::deserialize(payload)?;
            Ok(DecodedPacket::Input(packet))
        }
        PacketType::Control => {
            let packet: ControlPacket = bincode::deserialize(payload)?;
            Ok(DecodedPacket::Control(packet))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_video_packet_roundtrip() {
        let packet = VideoPacket {
            sequence: 42,
            timestamp: 123456,
            is_keyframe: true,
            fragment_index: 2,
            fragment_count: 5,
            data: vec![0x00, 0x00, 0x01, 0x67],
        };

        let encoded = encode_video(&packet).unwrap();
        let decoded = decode_packet(&encoded).unwrap();

        match decoded {
            DecodedPacket::Video(p) => {
                assert_eq!(p.sequence, 42);
                assert_eq!(p.timestamp, 123456);
                assert!(p.is_keyframe);
                assert_eq!(p.fragment_index, 2);
                assert_eq!(p.fragment_count, 5);
                assert_eq!(p.data, vec![0x00, 0x00, 0x01, 0x67]);
            }
            _ => panic!("expected video packet"),
        }
    }

    #[test]
    fn test_input_packet_roundtrip() {
        let packet = InputPacket {
            timestamp: 999,
            event: InputEvent::StylusDown {
                x: 0.5,
                y: 0.75,
                pressure: 0.8,
                tilt_x: 15.0,
                tilt_y: -10.0,
            },
        };

        let encoded = encode_input(&packet).unwrap();
        let decoded = decode_packet(&encoded).unwrap();

        match decoded {
            DecodedPacket::Input(p) => {
                assert_eq!(p.timestamp, 999);
                match p.event {
                    InputEvent::StylusDown { x, y, pressure, tilt_x, tilt_y } => {
                        assert!((x - 0.5).abs() < 0.001);
                        assert!((y - 0.75).abs() < 0.001);
                        assert!((pressure - 0.8).abs() < 0.001);
                        assert!((tilt_x - 15.0).abs() < 0.001);
                        assert!((tilt_y - (-10.0)).abs() < 0.001);
                    }
                    _ => panic!("expected stylus down"),
                }
            }
            _ => panic!("expected input packet"),
        }
    }

    #[test]
    fn test_control_packet_roundtrip() {
        let packet = ControlPacket::ConnectAck {
            width: 1920,
            height: 1080,
        };

        let encoded = encode_control(&packet).unwrap();
        let decoded = decode_packet(&encoded).unwrap();

        match decoded {
            DecodedPacket::Control(ControlPacket::ConnectAck { width, height }) => {
                assert_eq!(width, 1920);
                assert_eq!(height, 1080);
            }
            _ => panic!("expected connect ack"),
        }
    }
}
