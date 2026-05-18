//! CONTINUATION frame.
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
    FrameHeader {
        length: header_block_length,
        frame_type: FrameType::Continuation as u8,
        flags: if end_headers { FLAG_END_HEADERS } else { 0 },
        stream_id,
    }
    .encode(buf);
    Some(FRAME_HEADER_LEN)
}

#[cfg(test)]
mod tests {
    use super::{
        super::{Frame, FrameDecodeError, FrameType, encode_frame},
        *,
    };
    use crate::h2::H2ErrorCode;

    #[test]
    fn continuation_roundtrip() {
        let block = b"continued-fragment";
        let prefix_len = ENCODED_PREFIX_LEN;
        let mut buf = vec![0u8; prefix_len + block.len()];
        encode_prefix(9, true, block.len() as u32, &mut buf).unwrap();
        buf[prefix_len..].copy_from_slice(block);

        let (frame, consumed) = Frame::decode(&buf).unwrap();
        assert_eq!(consumed, prefix_len);
        assert_eq!(
            frame,
            Frame::Continuation {
                stream_id: 9,
                end_headers: true,
                header_block_length: block.len() as u32,
            }
        );
    }

    #[test]
    fn continuation_on_stream_zero_protocol_error() {
        let buf = encode_frame(FrameType::Continuation, 0, 0, b"");
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
        );
    }
}
