//! SETTINGS frame (RFC 9113 §6.5).

use super::{FLAG_ACK, FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader, FrameType};
use crate::h2::{H2ErrorCode, H2Settings};

pub(crate) fn decode(header: FrameHeader, payload: &[u8]) -> Result<Frame, FrameDecodeError> {
    if header.stream_id != 0 {
        return Err(H2ErrorCode::ProtocolError.into());
    }
    if header.flags & FLAG_ACK != 0 {
        if !payload.is_empty() {
            return Err(H2ErrorCode::FrameSizeError.into());
        }
        Ok(Frame::SettingsAck)
    } else {
        Ok(Frame::Settings(H2Settings::decode(payload)?))
    }
}

pub(crate) fn encoded_len(settings: &H2Settings) -> usize {
    FRAME_HEADER_LEN + settings.encoded_len()
}

pub(crate) fn encode(settings: &H2Settings, buf: &mut [u8]) -> Option<usize> {
    let payload_len = settings.encoded_len();
    let total = FRAME_HEADER_LEN + payload_len;
    if buf.len() < total {
        return None;
    }
    FrameHeader {
        length: u32::try_from(payload_len).expect("settings payload fits in 24 bits"),
        frame_type: FrameType::Settings as u8,
        flags: 0,
        stream_id: 0,
    }
    .encode(buf);
    settings.encode(&mut buf[FRAME_HEADER_LEN..])?;
    Some(total)
}

pub(crate) fn encode_ack(buf: &mut [u8]) -> Option<usize> {
    if buf.len() < FRAME_HEADER_LEN {
        return None;
    }
    FrameHeader {
        length: 0,
        frame_type: FrameType::Settings as u8,
        flags: FLAG_ACK,
        stream_id: 0,
    }
    .encode(buf);
    Some(FRAME_HEADER_LEN)
}

pub(crate) const ACK_ENCODED_LEN: usize = FRAME_HEADER_LEN;

#[cfg(test)]
mod tests {
    use super::{
        super::{FLAG_ACK, FRAME_HEADER_LEN, Frame, FrameDecodeError, FrameHeader, FrameType},
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
    fn settings_roundtrip_body() {
        let settings = H2Settings::server_defaults().with_header_table_size(4096);
        let mut buf = vec![0u8; encoded_len(&settings)];
        let len = encode(&settings, &mut buf).unwrap();
        let (frame, consumed) = Frame::decode(&buf[..len]).unwrap();
        assert_eq!(consumed, len);
        assert_eq!(frame, Frame::Settings(settings));
    }

    #[test]
    fn settings_ack_roundtrip() {
        let mut buf = vec![0u8; ACK_ENCODED_LEN];
        let len = encode_ack(&mut buf).unwrap();
        let (frame, consumed) = Frame::decode(&buf[..len]).unwrap();
        assert_eq!(consumed, len);
        assert_eq!(frame, Frame::SettingsAck);
    }

    #[test]
    fn settings_ack_with_payload_is_frame_size_error() {
        let buf = encode_frame(FrameType::Settings, FLAG_ACK, 0, &[0u8; 6]);
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::FrameSizeError)),
        );
    }

    #[test]
    fn settings_on_nonzero_stream_is_protocol_error() {
        let buf = encode_frame(FrameType::Settings, 0, 1, &[]);
        assert_eq!(
            Frame::decode(&buf),
            Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
        );
    }
}
