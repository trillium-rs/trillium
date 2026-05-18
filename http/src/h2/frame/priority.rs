//! PRIORITY frame. Deprecated by RFC 9113 — parse and discard.

use super::{Frame, FrameDecodeError, FrameHeader, PriorityInfo};
use crate::h2::H2ErrorCode;

pub(crate) fn decode(header: FrameHeader, payload: &[u8]) -> Result<Frame, FrameDecodeError> {
    if header.stream_id == 0 {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    if payload.len() != PriorityInfo::WIRE_LEN as usize {
        return Err(H2ErrorCode::FrameSizeError.into());
    }
    // Surface the parsed block — the scheme is deprecated so we don't use it for
    // priority decisions, but the connection layer still needs to see the priority
    // info to reject self-dependency.
    Ok(Frame::Priority {
        stream_id: header.stream_id,
        priority: PriorityInfo::decode(payload),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        super::{Frame, FrameDecodeError, FrameType, encode_frame},
        *,
    };
    use crate::h2::H2ErrorCode;

    #[test]
    fn priority_parse_and_surface() {
        // exclusive=1, dep=3, weight=16 (wire byte = 15)
        let mut payload = [0u8; 5];
        payload[0..4].copy_from_slice(&(0x8000_0003u32).to_be_bytes());
        payload[4] = 15;
        let buf = encode_frame(FrameType::Priority, 0, 7, &payload);
        let (frame, _) = Frame::decode(&buf).unwrap();
        assert_eq!(
            frame,
            Frame::Priority {
                stream_id: 7,
                priority: PriorityInfo {
                    exclusive: true,
                    stream_dependency: 3,
                    weight: 16,
                },
            },
        );
    }

    #[test]
    fn priority_on_stream_zero_protocol_error() {
        let buf = encode_frame(FrameType::Priority, 0, 0, &[0u8; 5]);
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
        );
    }

    #[test]
    fn priority_wrong_size_frame_size_error() {
        let buf = encode_frame(FrameType::Priority, 0, 1, &[0u8; 4]);
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::FrameSizeError)),
        );
    }
}
