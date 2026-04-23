//! `RST_STREAM` frame (RFC 9113 §6.4).

use super::{FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader, FrameType};
use crate::h2::H2ErrorCode;

const PAYLOAD_LEN: u32 = 4;
pub(crate) const ENCODED_LEN: usize = FRAME_HEADER_LEN + PAYLOAD_LEN as usize;

pub(crate) fn decode(header: FrameHeader, payload: &[u8]) -> Result<Frame, FrameDecodeError> {
    if header.stream_id == 0 {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    if payload.len() != PAYLOAD_LEN as usize {
        return Err(H2ErrorCode::FrameSizeError.into());
    }
    let error_code =
        u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]).into();
    Ok(Frame::RstStream {
        stream_id: header.stream_id,
        error_code,
    })
}

pub(crate) fn encode(
    stream_id: u32,
    error_code: H2ErrorCode,
    buf: &mut [u8],
) -> Option<usize> {
    if buf.len() < ENCODED_LEN {
        return None;
    }
    let (header_buf, payload_buf) = buf.split_at_mut(FRAME_HEADER_LEN);
    FrameHeader {
        length: PAYLOAD_LEN,
        frame_type: FrameType::RstStream as u8,
        flags: 0,
        stream_id,
    }
    .encode(header_buf.try_into().expect("split_at_mut slot"));
    payload_buf[..PAYLOAD_LEN as usize]
        .copy_from_slice(&u32::from(error_code).to_be_bytes());
    Some(ENCODED_LEN)
}
