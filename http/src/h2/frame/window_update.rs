//! `WINDOW_UPDATE` frame (RFC 9113 §6.9).

use super::{FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader, FrameType};
use crate::h2::H2ErrorCode;

const PAYLOAD_LEN: u32 = 4;
pub(crate) const ENCODED_LEN: usize = FRAME_HEADER_LEN + PAYLOAD_LEN as usize;

pub(crate) fn decode(header: FrameHeader, payload: &[u8]) -> Result<Frame, FrameDecodeError> {
    if payload.len() != PAYLOAD_LEN as usize {
        return Err(H2ErrorCode::FrameSizeError.into());
    }
    // Top bit is reserved and MUST be ignored.
    let increment =
        u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) & 0x7FFF_FFFF;
    // §6.9: a 0 increment on the connection is a connection error; on a stream it's a stream
    // error. Report as ProtocolError; the caller classifies using stream_id.
    if increment == 0 {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    Ok(Frame::WindowUpdate {
        stream_id: header.stream_id,
        increment,
    })
}

pub(crate) fn encode(stream_id: u32, increment: u32, buf: &mut [u8]) -> Option<usize> {
    if buf.len() < ENCODED_LEN {
        return None;
    }
    FrameHeader {
        length: PAYLOAD_LEN,
        frame_type: FrameType::WindowUpdate as u8,
        flags: 0,
        stream_id,
    }
    .encode(buf);
    buf[FRAME_HEADER_LEN..ENCODED_LEN].copy_from_slice(&(increment & 0x7FFF_FFFF).to_be_bytes());
    Some(ENCODED_LEN)
}

#[cfg(test)]
mod tests {
    use super::{
        super::{FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader, FrameType},
        *,
    };
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
    fn window_update_roundtrip() {
        let mut buf = vec![0u8; ENCODED_LEN];
        let len = encode(1, 65535, &mut buf).unwrap();
        let (frame, consumed) = Frame::decode(&buf[..len]).unwrap();
        assert_eq!(consumed, len);
        assert_eq!(
            frame,
            Frame::WindowUpdate {
                stream_id: 1,
                increment: 65535,
            }
        );
    }

    #[test]
    fn window_update_zero_increment_is_protocol_error() {
        let buf = encode_frame(FrameType::WindowUpdate, 0, 1, &[0, 0, 0, 0]);
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
        );
    }

    #[test]
    fn window_update_reserved_bit_ignored() {
        let buf = encode_frame(FrameType::WindowUpdate, 0, 0, &[0xFF, 0xFF, 0xFF, 0xFF]);
        let (frame, _) = Frame::decode(&buf).unwrap();
        assert_eq!(
            frame,
            Frame::WindowUpdate {
                stream_id: 0,
                increment: 0x7FFF_FFFF,
            }
        );
    }
}
