//! `PRIORITY_UPDATE` frame (RFC 9218) — reprioritizes a request stream.

use super::{Frame, FrameDecodeError, FrameHeader};
use crate::{Priority, h2::H2ErrorCode};

const PRIORITIZED_STREAM_ID_LEN: usize = 4;

pub(crate) fn decode(header: FrameHeader, payload: &[u8]) -> Result<Frame, FrameDecodeError> {
    // PRIORITY_UPDATE travels on the connection control stream; a non-zero frame stream
    // id is a connection error.
    if header.stream_id != 0 {
        return Err(H2ErrorCode::ProtocolError.into());
    }

    let id_bytes = payload
        .get(..PRIORITIZED_STREAM_ID_LEN)
        .ok_or(H2ErrorCode::FrameSizeError)?;
    let prioritized_stream_id =
        u32::from_be_bytes([id_bytes[0], id_bytes[1], id_bytes[2], id_bytes[3]]) & 0x7FFF_FFFF;

    // The remaining payload is the Priority Field Value. An empty value, non-ASCII bytes,
    // or any malformed field all resolve to the default priority per the scheme's
    // graceful-degradation rule.
    let priority = std::str::from_utf8(&payload[PRIORITIZED_STREAM_ID_LEN..])
        .ok()
        .and_then(|field| field.parse::<Priority>().ok())
        .unwrap_or_default();

    Ok(Frame::PriorityUpdate {
        prioritized_stream_id,
        priority,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{Frame, FrameDecodeError, FrameType, encode_frame};
    use crate::{Priority, h2::H2ErrorCode};

    fn payload(stream_id: u32, field_value: &[u8]) -> Vec<u8> {
        let mut p = (stream_id & 0x7FFF_FFFF).to_be_bytes().to_vec();
        p.extend_from_slice(field_value);
        p
    }

    #[test]
    fn decodes_stream_id_and_priority() {
        let buf = encode_frame(FrameType::PriorityUpdate, 0, 0, &payload(3, b"u=1, i"));
        let (frame, consumed) = Frame::decode(&buf).unwrap();
        assert_eq!(consumed, buf.len());
        assert_eq!(
            frame,
            Frame::PriorityUpdate {
                prioritized_stream_id: 3,
                priority: Priority::new(1).with_incremental(true),
            }
        );
    }

    #[test]
    fn empty_field_value_is_default_priority() {
        let buf = encode_frame(FrameType::PriorityUpdate, 0, 0, &payload(7, b""));
        let (frame, _) = Frame::decode(&buf).unwrap();
        assert_eq!(
            frame,
            Frame::PriorityUpdate {
                prioritized_stream_id: 7,
                priority: Priority::default(),
            }
        );
    }

    #[test]
    fn reserved_bit_masked_off_prioritized_id() {
        let buf = encode_frame(
            FrameType::PriorityUpdate,
            0,
            0,
            &payload(0xFFFF_FFFF, b"u=0"),
        );
        let (frame, _) = Frame::decode(&buf).unwrap();
        assert_eq!(
            frame,
            Frame::PriorityUpdate {
                prioritized_stream_id: 0x7FFF_FFFF,
                priority: Priority::new(0),
            }
        );
    }

    #[test]
    fn nonzero_frame_stream_id_is_protocol_error() {
        let buf = encode_frame(FrameType::PriorityUpdate, 0, 1, &payload(3, b"u=1"));
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
        );
    }

    #[test]
    fn truncated_prioritized_id_is_frame_size_error() {
        let buf = encode_frame(FrameType::PriorityUpdate, 0, 0, &[0, 0, 0]);
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::FrameSizeError)),
        );
    }
}
