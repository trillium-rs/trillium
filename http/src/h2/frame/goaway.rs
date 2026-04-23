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
    let (header_buf, rest) = buf.split_at_mut(FRAME_HEADER_LEN);
    FrameHeader {
        length: u32::try_from(payload_len).expect("goaway payload fits in 24 bits"),
        frame_type: FrameType::Goaway as u8,
        flags: 0,
        stream_id: 0,
    }
    .encode(header_buf.try_into().expect("split_at_mut slot"));
    rest[0..4].copy_from_slice(&(last_stream_id & 0x7FFF_FFFF).to_be_bytes());
    rest[4..8].copy_from_slice(&u32::from(error_code).to_be_bytes());
    rest[8..8 + debug_data.len()].copy_from_slice(debug_data);
    Some(total)
}
