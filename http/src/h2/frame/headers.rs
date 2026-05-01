//! HEADERS frame (RFC 9113 §6.2).
//!
//! Decode consumes the 9-byte header plus any PADDED pad-length byte and/or PRIORITY 5-byte
//! block; the `header_block_length` field-fragment bytes and `padding_length` padding bytes stay
//! in the caller's input slice for streaming. Encode mirrors that — the caller writes the header
//! block fragment and padding.

use super::{
    FLAG_END_HEADERS, FLAG_END_STREAM, FLAG_PADDED, FLAG_PRIORITY, FRAME_HEADER_LEN, Frame,
    FrameDecodeError, FrameHeader, FrameType, PriorityInfo,
};
use crate::h2::H2ErrorCode;

pub(crate) fn decode_prefix(
    header: FrameHeader,
    prefix_input: &[u8],
) -> Result<(Frame, usize), FrameDecodeError> {
    if header.stream_id == 0 {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    let padded = header.flags & FLAG_PADDED != 0;
    let priority_flag = header.flags & FLAG_PRIORITY != 0;
    let mut cursor: u32 = 0;

    let padding_length = if padded {
        let pad_length = *prefix_input.first().ok_or(FrameDecodeError::Incomplete)?;
        cursor += 1;
        pad_length
    } else {
        0
    };

    let priority = if priority_flag {
        let slice = prefix_input
            .get(cursor as usize..cursor as usize + PriorityInfo::WIRE_LEN as usize)
            .ok_or(FrameDecodeError::Incomplete)?;
        cursor += PriorityInfo::WIRE_LEN;
        Some(PriorityInfo::decode(slice))
    } else {
        None
    };

    let consumed = cursor + u32::from(padding_length);
    if consumed > header.length {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    let header_block_length = header.length - consumed;
    Ok((
        Frame::Headers {
            stream_id: header.stream_id,
            end_stream: header.flags & FLAG_END_STREAM != 0,
            end_headers: header.flags & FLAG_END_HEADERS != 0,
            priority,
            header_block_length,
            padding_length,
        },
        cursor as usize,
    ))
}

/// The size of the encoded prefix: 9-byte header + pad-length byte (if padded) + priority block
/// (if present).
pub(crate) fn encoded_prefix_len(padding_length: u8, has_priority: bool) -> usize {
    FRAME_HEADER_LEN
        + usize::from(padding_length > 0)
        + if has_priority {
            PriorityInfo::WIRE_LEN as usize
        } else {
            0
        }
}

/// Write the frame prefix (9-byte header + optional pad-length + optional priority) into `buf`.
/// The caller writes the `header_block_length` header-block bytes and `padding_length` padding
/// bytes that follow.
///
/// `trillium-http` never emits the deprecated PRIORITY flag itself; `priority` is wired up for
/// completeness and covered by tests, but server code paths should always pass `None`.
pub(crate) fn encode_prefix(
    stream_id: u32,
    end_stream: bool,
    end_headers: bool,
    priority: Option<PriorityInfo>,
    header_block_length: u32,
    padding_length: u8,
    buf: &mut [u8],
) -> Option<usize> {
    let has_priority = priority.is_some();
    let prefix_len = encoded_prefix_len(padding_length, has_priority);
    if buf.len() < prefix_len {
        return None;
    }
    let padded = padding_length > 0;
    let payload_length = header_block_length
        + u32::from(padding_length)
        + u32::from(padded)
        + if has_priority {
            PriorityInfo::WIRE_LEN
        } else {
            0
        };
    let mut flags = 0;
    if end_stream {
        flags |= FLAG_END_STREAM;
    }
    if end_headers {
        flags |= FLAG_END_HEADERS;
    }
    if padded {
        flags |= FLAG_PADDED;
    }
    if has_priority {
        flags |= FLAG_PRIORITY;
    }
    FrameHeader {
        length: payload_length,
        frame_type: FrameType::Headers as u8,
        flags,
        stream_id,
    }
    .encode(buf);
    let mut cursor = FRAME_HEADER_LEN;
    if padded {
        buf[cursor] = padding_length;
        cursor += 1;
    }
    if let Some(priority) = priority {
        let dep = priority.stream_dependency & 0x7FFF_FFFF
            | if priority.exclusive { 0x8000_0000 } else { 0 };
        buf[cursor..cursor + 4].copy_from_slice(&dep.to_be_bytes());
        buf[cursor + 4] =
            u8::try_from(priority.weight.saturating_sub(1)).expect("priority weight is 1..=256");
    }
    Some(prefix_len)
}

#[cfg(test)]
mod tests {
    use super::{
        super::{FLAG_PRIORITY, Frame, FrameDecodeError, FrameType, encode_frame},
        *,
    };
    use crate::h2::H2ErrorCode;

    #[test]
    fn headers_roundtrip_plain() {
        let block = b"\x00\x00abc";
        let prefix_len = encoded_prefix_len(0, false);
        let mut buf = vec![0u8; prefix_len + block.len()];
        encode_prefix(7, false, true, None, block.len() as u32, 0, &mut buf).unwrap();
        buf[prefix_len..].copy_from_slice(block);

        let (frame, consumed) = Frame::decode(&buf).unwrap();
        assert_eq!(consumed, prefix_len);
        assert_eq!(
            frame,
            Frame::Headers {
                stream_id: 7,
                end_stream: false,
                end_headers: true,
                priority: None,
                header_block_length: block.len() as u32,
                padding_length: 0,
            }
        );
    }

    #[test]
    fn headers_roundtrip_padded_priority_and_end_stream() {
        let block = b"some-header-block";
        let padding = 3u8;
        let priority = PriorityInfo {
            exclusive: true,
            stream_dependency: 11,
            weight: 42,
        };
        let prefix_len = encoded_prefix_len(padding, true);
        let mut buf = vec![0u8; prefix_len + block.len() + padding as usize];
        encode_prefix(
            13,
            true,
            true,
            Some(priority),
            block.len() as u32,
            padding,
            &mut buf,
        )
        .unwrap();
        buf[prefix_len..prefix_len + block.len()].copy_from_slice(block);

        let (frame, consumed) = Frame::decode(&buf).unwrap();
        assert_eq!(consumed, prefix_len);
        assert_eq!(
            frame,
            Frame::Headers {
                stream_id: 13,
                end_stream: true,
                end_headers: true,
                priority: Some(priority),
                header_block_length: block.len() as u32,
                padding_length: padding,
            }
        );
    }

    #[test]
    fn headers_on_stream_zero_protocol_error() {
        let buf = encode_frame(FrameType::Headers, 0, 0, b"xyz");
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
        );
    }

    #[test]
    fn headers_priority_prefix_without_enough_bytes_is_incomplete() {
        // PRIORITY flag set but frame payload is only 4 bytes — priority block needs 5.
        let buf = encode_frame(FrameType::Headers, FLAG_PRIORITY, 1, &[0u8; 4]);
        assert_eq!(Frame::decode(&buf), Err(FrameDecodeError::Incomplete));
    }
}
