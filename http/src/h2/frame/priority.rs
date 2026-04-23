//! PRIORITY frame (RFC 9113 §6.3). Deprecated (§5.3.2) — parse and discard.

use super::{Frame, FrameDecodeError, FrameHeader, PriorityInfo};
use crate::h2::H2ErrorCode;

pub(crate) fn decode(header: FrameHeader, payload: &[u8]) -> Result<Frame, FrameDecodeError> {
    if header.stream_id == 0 {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    if payload.len() != PriorityInfo::WIRE_LEN as usize {
        return Err(H2ErrorCode::FrameSizeError.into());
    }
    // Surface the parsed block — the scheme is deprecated (§5.3.2) so we don't use it for
    // priority decisions, but §5.3.1 requires us to reject self-dependency, which means
    // the connection layer has to see the priority info.
    Ok(Frame::Priority {
        stream_id: header.stream_id,
        priority: PriorityInfo::decode(payload),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        super::{FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader, FrameType},
        *,
    };
    use crate::h2::H2ErrorCode;

    fn encode_frame(frame_type: FrameType, flags: u8, stream_id: u32, payload: &[u8]) -> Vec<u8> {
        let mut buf = vec![0u8; FRAME_HEADER_LEN + payload.len()];
        FrameHeader {
            length: u32::try_from(payload.len()).unwrap(),
            frame_type: frame_type as u8,
            flags,
            stream_id,
        }
        .encode((&mut buf[..FRAME_HEADER_LEN]).try_into().unwrap());
        buf[FRAME_HEADER_LEN..].copy_from_slice(payload);
        buf
    }

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
