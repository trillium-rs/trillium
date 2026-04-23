//! PING frame (RFC 9113 §6.7).

use super::{FLAG_ACK, FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader, FrameType};
use crate::h2::H2ErrorCode;

const PAYLOAD_LEN: u32 = 8;
pub(crate) const ENCODED_LEN: usize = FRAME_HEADER_LEN + PAYLOAD_LEN as usize;

pub(crate) fn decode(header: FrameHeader, payload: &[u8]) -> Result<Frame, FrameDecodeError> {
    if header.stream_id != 0 {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    if payload.len() != PAYLOAD_LEN as usize {
        return Err(H2ErrorCode::FrameSizeError.into());
    }
    let mut opaque_data = [0u8; PAYLOAD_LEN as usize];
    opaque_data.copy_from_slice(payload);
    Ok(Frame::Ping {
        opaque_data,
        ack: header.flags & FLAG_ACK != 0,
    })
}

pub(crate) fn encode(
    opaque_data: [u8; PAYLOAD_LEN as usize],
    ack: bool,
    buf: &mut [u8],
) -> Option<usize> {
    if buf.len() < ENCODED_LEN {
        return None;
    }
    let (header_buf, payload_buf) = buf.split_at_mut(FRAME_HEADER_LEN);
    FrameHeader {
        length: PAYLOAD_LEN,
        frame_type: FrameType::Ping as u8,
        flags: if ack { FLAG_ACK } else { 0 },
        stream_id: 0,
    }
    .encode(header_buf.try_into().expect("split_at_mut slot"));
    payload_buf[..PAYLOAD_LEN as usize].copy_from_slice(&opaque_data);
    Some(ENCODED_LEN)
}
