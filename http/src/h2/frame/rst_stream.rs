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
    let error_code = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]).into();
    Ok(Frame::RstStream {
        stream_id: header.stream_id,
        error_code,
    })
}

pub(crate) fn encode(stream_id: u32, error_code: H2ErrorCode, buf: &mut [u8]) -> Option<usize> {
    if buf.len() < ENCODED_LEN {
        return None;
    }
    FrameHeader {
        length: PAYLOAD_LEN,
        frame_type: FrameType::RstStream as u8,
        flags: 0,
        stream_id,
    }
    .encode(buf);
    buf[FRAME_HEADER_LEN..ENCODED_LEN].copy_from_slice(&u32::from(error_code).to_be_bytes());
    Some(ENCODED_LEN)
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
    fn rst_stream_roundtrip() {
        let mut buf = vec![0u8; ENCODED_LEN];
        let len = encode(7, H2ErrorCode::Cancel, &mut buf).unwrap();
        let (frame, consumed) = Frame::decode(&buf[..len]).unwrap();
        assert_eq!(consumed, len);
        assert_eq!(
            frame,
            Frame::RstStream {
                stream_id: 7,
                error_code: H2ErrorCode::Cancel,
            }
        );
    }

    #[test]
    fn rst_stream_on_stream_zero_protocol_error() {
        let buf = encode_frame(FrameType::RstStream, 0, 0, &[0, 0, 0, 0]);
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
        );
    }

    #[test]
    fn rst_stream_unknown_error_code_decodes_as_no_error() {
        let buf = encode_frame(FrameType::RstStream, 0, 1, &[0xff, 0xff, 0xff, 0xff]);
        let (frame, _) = Frame::decode(&buf).unwrap();
        assert_eq!(
            frame,
            Frame::RstStream {
                stream_id: 1,
                error_code: H2ErrorCode::NoError,
            }
        );
    }
}
