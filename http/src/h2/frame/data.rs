//! DATA frame (RFC 9113 §6.1).
//!
//! Decode consumes only the 9-byte header plus the optional pad-length byte; the `data_length`
//! data bytes and `padding_length` padding bytes remain in the caller's input slice for streaming.
//! Encode writes the same prefix; the caller then writes the data and padding.

use super::{
    FLAG_END_STREAM, FLAG_PADDED, FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader, FrameType,
};
use crate::h2::H2ErrorCode;

/// Decode the frame prefix from `prefix_input` (the bytes immediately after the 9-byte header).
/// Returns the frame plus the number of prefix bytes consumed (0 or 1, depending on `PADDED`).
pub(crate) fn decode_prefix(
    header: FrameHeader,
    prefix_input: &[u8],
) -> Result<(Frame, usize), FrameDecodeError> {
    if header.stream_id == 0 {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    let padded = header.flags & FLAG_PADDED != 0;
    let (padding_length, prefix_len) = if padded {
        let pad_length = *prefix_input.first().ok_or(FrameDecodeError::Incomplete)?;
        if u32::from(pad_length) >= header.length {
            // RFC 9113 §6.1: pad length ≥ rest of payload ⇒ PROTOCOL_ERROR
            return Err(H2ErrorCode::ProtocolError.into());
        }
        (pad_length, 1u32)
    } else {
        (0, 0)
    };
    let data_length = header.length - u32::from(padding_length) - prefix_len;
    Ok((
        Frame::Data {
            stream_id: header.stream_id,
            end_stream: header.flags & FLAG_END_STREAM != 0,
            data_length,
            padding_length,
        },
        prefix_len as usize,
    ))
}

/// The size of the encoded prefix (9-byte header plus optional pad-length byte).
pub(crate) fn encoded_prefix_len(padding_length: u8) -> usize {
    FRAME_HEADER_LEN + usize::from(padding_length > 0)
}

/// Write the 9-byte frame header (and pad-length byte if padded) into `buf`. The caller is
/// responsible for writing the `data_length` payload bytes and `padding_length` zero padding
/// bytes that follow.
///
/// Returns the number of bytes written, or `None` if `buf` is too small.
pub(crate) fn encode_prefix(
    stream_id: u32,
    end_stream: bool,
    data_length: u32,
    padding_length: u8,
    buf: &mut [u8],
) -> Option<usize> {
    let prefix_len = encoded_prefix_len(padding_length);
    if buf.len() < prefix_len {
        return None;
    }
    let padded = padding_length > 0;
    let payload_length = data_length + u32::from(padding_length) + u32::from(padded);
    let mut flags = 0;
    if end_stream {
        flags |= FLAG_END_STREAM;
    }
    if padded {
        flags |= FLAG_PADDED;
    }
    FrameHeader {
        length: payload_length,
        frame_type: FrameType::Data as u8,
        flags,
        stream_id,
    }
    .encode(buf);
    if padded {
        buf[FRAME_HEADER_LEN] = padding_length;
    }
    Some(prefix_len)
}
