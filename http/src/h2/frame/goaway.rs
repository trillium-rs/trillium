//! GOAWAY frame (RFC 9113 §6.8).

use super::{FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader, FrameType};
use crate::h2::H2ErrorCode;

/// Fixed prefix: last-stream-id (4) + error-code (4). Debug data follows.
const FIXED_PREFIX_LEN: usize = 8;

pub(crate) fn decode(header: FrameHeader, payload: &[u8]) -> Result<Frame, FrameDecodeError> {
    if header.stream_id != 0 {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    if payload.len() < FIXED_PREFIX_LEN {
        return Err(H2ErrorCode::FrameSizeError.into());
    }
    let last_stream_id =
        u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) & 0x7FFF_FFFF;
    let error_code = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]).into();
    let debug_data_length = u32::try_from(payload.len() - FIXED_PREFIX_LEN)
        .expect("goaway payload length came from a u32 frame header");
    Ok(Frame::Goaway {
        last_stream_id,
        error_code,
        debug_data_length,
    })
}

pub(crate) fn encoded_len(debug_data_len: usize) -> usize {
    FRAME_HEADER_LEN + FIXED_PREFIX_LEN + debug_data_len
}

pub(crate) fn encode(
    last_stream_id: u32,
    error_code: H2ErrorCode,
    debug_data: &[u8],
    buf: &mut [u8],
) -> Option<usize> {
    let total = encoded_len(debug_data.len());
    if buf.len() < total {
        return None;
    }
    let payload_len = FIXED_PREFIX_LEN + debug_data.len();
    FrameHeader {
        length: u32::try_from(payload_len).expect("goaway payload fits in 24 bits"),
        frame_type: FrameType::Goaway as u8,
        flags: 0,
        stream_id: 0,
    }
    .encode(buf);
    let payload = &mut buf[FRAME_HEADER_LEN..];
    payload[0..4].copy_from_slice(&(last_stream_id & 0x7FFF_FFFF).to_be_bytes());
    payload[4..8].copy_from_slice(&u32::from(error_code).to_be_bytes());
    payload[8..8 + debug_data.len()].copy_from_slice(debug_data);
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::super::{Frame, FrameDecodeError, FrameHeader, FrameType, FRAME_HEADER_LEN};
    use super::*;
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
    fn goaway_roundtrip_with_debug_data() {
        let mut buf = vec![0u8; encoded_len(5)];
        let len = encode(42, H2ErrorCode::InternalError, b"hello", &mut buf).unwrap();
        let (frame, consumed) = Frame::decode(&buf[..len]).unwrap();
        assert_eq!(consumed, len);
        assert_eq!(
            frame,
            Frame::Goaway {
                last_stream_id: 42,
                error_code: H2ErrorCode::InternalError,
                debug_data_length: 5,
            }
        );
        // Debug data is after the fixed prefix at the tail of the frame.
        assert_eq!(&buf[FRAME_HEADER_LEN + 8..len], b"hello");
    }

    #[test]
    fn goaway_short_payload_is_frame_size_error() {
        let buf = encode_frame(FrameType::Goaway, 0, 0, &[0u8; 7]);
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::FrameSizeError)),
        );
    }

    #[test]
    fn goaway_on_nonzero_stream_is_protocol_error() {
        let buf = encode_frame(FrameType::Goaway, 0, 3, &[0u8; 8]);
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
        );
    }
}
