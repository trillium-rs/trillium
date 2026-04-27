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
    FrameHeader {
        length: PAYLOAD_LEN,
        frame_type: FrameType::Ping as u8,
        flags: if ack { FLAG_ACK } else { 0 },
        stream_id: 0,
    }
    .encode(buf);
    buf[FRAME_HEADER_LEN..ENCODED_LEN].copy_from_slice(&opaque_data);
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
    fn ping_roundtrip_and_ack_roundtrip() {
        for ack in [false, true] {
            let opaque = [1u8, 2, 3, 4, 5, 6, 7, 8];
            let mut buf = vec![0u8; ENCODED_LEN];
            let len = encode(opaque, ack, &mut buf).unwrap();
            assert_eq!(len, ENCODED_LEN);

            let (frame, consumed) = Frame::decode(&buf[..len]).unwrap();
            assert_eq!(consumed, len);
            assert_eq!(
                frame,
                Frame::Ping {
                    opaque_data: opaque,
                    ack,
                }
            );
        }
    }

    #[test]
    fn ping_wrong_stream_id_protocol_error() {
        let buf = encode_frame(FrameType::Ping, 0, 1, &[0u8; 8]);
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
        );
    }

    #[test]
    fn ping_wrong_payload_size_frame_size_error() {
        let buf = encode_frame(FrameType::Ping, 0, 0, &[0u8; 7]);
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::FrameSizeError)),
        );
    }
}
