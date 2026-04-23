//! CONTINUATION frame (RFC 9113 §6.10).
//!
//! Decode returns the `header_block_length` field-fragment length; the fragment bytes stay in
//! the caller's input slice. Encode writes only the 9-byte header.

use super::{FLAG_END_HEADERS, FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader, FrameType};
use crate::h2::H2ErrorCode;

pub(crate) fn decode(header: FrameHeader) -> Result<Frame, FrameDecodeError> {
    if header.stream_id == 0 {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    Ok(Frame::Continuation {
        stream_id: header.stream_id,
        end_headers: header.flags & FLAG_END_HEADERS != 0,
        header_block_length: header.length,
    })
}

pub(crate) const ENCODED_PREFIX_LEN: usize = FRAME_HEADER_LEN;

pub(crate) fn encode_prefix(
    stream_id: u32,
    end_headers: bool,
    header_block_length: u32,
    buf: &mut [u8],
) -> Option<usize> {
    if buf.len() < FRAME_HEADER_LEN {
        return None;
    }
    let (header_buf, _) = buf.split_at_mut(FRAME_HEADER_LEN);
    FrameHeader {
        length: header_block_length,
        frame_type: FrameType::Continuation as u8,
        flags: if end_headers { FLAG_END_HEADERS } else { 0 },
        stream_id,
    }
    .encode(header_buf.try_into().expect("split_at_mut slot"));
    Some(FRAME_HEADER_LEN)
}
