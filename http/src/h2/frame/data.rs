//! DATA frame.
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

#[cfg(test)]
mod tests {
    use super::{
        super::{FLAG_PADDED, Frame, FrameDecodeError, FrameType, encode_frame},
        *,
    };
    use crate::h2::H2ErrorCode;

    #[test]
    fn data_roundtrip_plain() {
        let payload = b"hello world";
        let prefix_len = encoded_prefix_len(0);
        let mut buf = vec![0u8; prefix_len + payload.len()];
        let written = encode_prefix(3, false, payload.len() as u32, 0, &mut buf).unwrap();
        assert_eq!(written, prefix_len);
        buf[prefix_len..].copy_from_slice(payload);

        let (frame, consumed) = Frame::decode(&buf).unwrap();
        assert_eq!(consumed, prefix_len);
        assert_eq!(
            frame,
            Frame::Data {
                stream_id: 3,
                end_stream: false,
                data_length: payload.len() as u32,
                padding_length: 0,
            }
        );
        assert_eq!(&buf[prefix_len..prefix_len + payload.len()], payload);
    }

    #[test]
    fn data_roundtrip_end_stream_flag() {
        let payload = b"goodbye";
        let prefix_len = encoded_prefix_len(0);
        let mut buf = vec![0u8; prefix_len + payload.len()];
        encode_prefix(1, true, payload.len() as u32, 0, &mut buf).unwrap();
        buf[prefix_len..].copy_from_slice(payload);
        let (frame, _) = Frame::decode(&buf).unwrap();
        assert_eq!(
            frame,
            Frame::Data {
                stream_id: 1,
                end_stream: true,
                data_length: payload.len() as u32,
                padding_length: 0,
            }
        );
    }

    #[test]
    fn data_roundtrip_padded() {
        let payload = b"padded-data";
        let padding = 4u8;
        let prefix_len = encoded_prefix_len(padding);
        let mut buf = vec![0u8; prefix_len + payload.len() + padding as usize];
        encode_prefix(5, false, payload.len() as u32, padding, &mut buf).unwrap();
        buf[prefix_len..prefix_len + payload.len()].copy_from_slice(payload);

        let (frame, consumed) = Frame::decode(&buf).unwrap();
        assert_eq!(consumed, prefix_len);
        assert_eq!(
            frame,
            Frame::Data {
                stream_id: 5,
                end_stream: false,
                data_length: payload.len() as u32,
                padding_length: padding,
            }
        );
    }

    #[test]
    fn data_on_stream_zero_protocol_error() {
        let buf = encode_frame(FrameType::Data, 0, 0, b"hi");
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
        );
    }

    #[test]
    fn data_pad_length_covering_entire_payload_is_protocol_error() {
        // PADDED set; payload length = 5; pad length byte = 5 ⇒ no room for data
        let payload = [5u8, 0, 0, 0, 0];
        let buf = encode_frame(FrameType::Data, FLAG_PADDED, 1, &payload);
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
        );
    }
}
