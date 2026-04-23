use super::*;
use crate::h2::{H2ErrorCode, H2Settings};

#[test]
fn frame_header_roundtrip() {
    let header = FrameHeader {
        length: 0x00_01_02_03 & 0x00FF_FFFF,
        frame_type: 0x09,
        flags: 0x0F,
        stream_id: 0x1234_5678,
    };
    let mut buf = [0u8; FRAME_HEADER_LEN];
    header.encode(&mut buf);
    let decoded = FrameHeader::decode(&buf).unwrap();
    assert_eq!(decoded, header);
}

#[test]
fn frame_header_masks_reserved_bit_on_decode() {
    let mut buf = [0u8; FRAME_HEADER_LEN];
    // length=1, type=6 (PING), flags=0, stream_id with reserved bit set
    buf[2] = 1;
    buf[3] = 0x06;
    buf[5..9].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
    let decoded = FrameHeader::decode(&buf).unwrap();
    assert_eq!(decoded.stream_id, 0x7FFF_FFFF);
}

#[test]
fn frame_header_incomplete() {
    assert!(FrameHeader::decode(&[0u8; FRAME_HEADER_LEN - 1]).is_none());
}

/// Helper: encode a control frame's header + payload into a single buffer.
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
        let mut buf = vec![0u8; ping::ENCODED_LEN];
        let len = ping::encode(opaque, ack, &mut buf).unwrap();
        assert_eq!(len, ping::ENCODED_LEN);

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

#[test]
fn rst_stream_roundtrip() {
    let mut buf = vec![0u8; rst_stream::ENCODED_LEN];
    let len = rst_stream::encode(7, H2ErrorCode::Cancel, &mut buf).unwrap();
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

#[test]
fn window_update_roundtrip() {
    let mut buf = vec![0u8; window_update::ENCODED_LEN];
    let len = window_update::encode(1, 65535, &mut buf).unwrap();
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

#[test]
fn settings_roundtrip_body() {
    let settings = H2Settings::server_defaults().with_header_table_size(4096);
    let mut buf = vec![0u8; settings::encoded_len(&settings)];
    let len = settings::encode(&settings, &mut buf).unwrap();
    let (frame, consumed) = Frame::decode(&buf[..len]).unwrap();
    assert_eq!(consumed, len);
    assert_eq!(frame, Frame::Settings(settings));
}

#[test]
fn settings_ack_roundtrip() {
    let mut buf = vec![0u8; settings::ACK_ENCODED_LEN];
    let len = settings::encode_ack(&mut buf).unwrap();
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

#[test]
fn goaway_roundtrip_with_debug_data() {
    let mut buf = vec![0u8; goaway::encoded_len(5)];
    let len = goaway::encode(42, H2ErrorCode::InternalError, b"hello", &mut buf).unwrap();
    let (frame, consumed) = Frame::decode(&buf[..len]).unwrap();
    assert_eq!(consumed, len);
    assert_eq!(
        frame,
        Frame::Goaway {
            last_stream_id: 42,
            error_code: H2ErrorCode::InternalError,
            debug_data_length: 5,
        }
    );
    // Debug data is after the fixed prefix at the tail of the frame.
    assert_eq!(&buf[FRAME_HEADER_LEN + 8..len], b"hello");
}

#[test]
fn goaway_short_payload_is_frame_size_error() {
    let buf = encode_frame(FrameType::Goaway, 0, 0, &[0u8; 7]);
    assert_eq!(
        Frame::decode(&buf),
        Err(FrameDecodeError::Error(H2ErrorCode::FrameSizeError)),
    );
}

#[test]
fn goaway_on_nonzero_stream_is_protocol_error() {
    let buf = encode_frame(FrameType::Goaway, 0, 3, &[0u8; 8]);
    assert_eq!(
        Frame::decode(&buf),
        Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
    );
}

#[test]
fn priority_parse_and_discard() {
    // exclusive=1, dep=3, weight=16 (wire byte = 15)
    let mut payload = [0u8; 5];
    payload[0..4].copy_from_slice(&(0x8000_0003u32).to_be_bytes());
    payload[4] = 15;
    let buf = encode_frame(FrameType::Priority, 0, 7, &payload);
    let (frame, _) = Frame::decode(&buf).unwrap();
    assert_eq!(frame, Frame::Priority { stream_id: 7 });
}

#[test]
fn priority_on_stream_zero_protocol_error() {
    let buf = encode_frame(FrameType::Priority, 0, 0, &[0u8; 5]);
    assert_eq!(
        Frame::decode(&buf),
        Err(FrameDecodeError::Error(H2ErrorCode::ProtocolError)),
    );
}

#[test]
fn priority_wrong_size_frame_size_error() {
    let buf = encode_frame(FrameType::Priority, 0, 1, &[0u8; 4]);
    assert_eq!(
        Frame::decode(&buf),
        Err(FrameDecodeError::Error(H2ErrorCode::FrameSizeError)),
    );
}

#[test]
fn unknown_frame_type_returns_unknown_variant() {
    let payload = [1u8, 2, 3];
    let mut buf = vec![0u8; FRAME_HEADER_LEN + payload.len()];
    FrameHeader {
        length: u32::try_from(payload.len()).unwrap(),
        frame_type: 0xBE,
        flags: 0xEF,
        stream_id: 5,
    }
    .encode((&mut buf[..FRAME_HEADER_LEN]).try_into().unwrap());
    buf[FRAME_HEADER_LEN..].copy_from_slice(&payload);

    let (frame, consumed) = Frame::decode(&buf).unwrap();
    // Only the header is consumed; payload stays in the slice for the caller to skip.
    assert_eq!(consumed, FRAME_HEADER_LEN);
    assert_eq!(
        frame,
        Frame::Unknown {
            stream_id: 5,
            frame_type: 0xBE,
            flags: 0xEF,
            length: 3,
        }
    );
}

#[test]
fn push_promise_variant_surfaced_for_rejection() {
    let payload = [0u8; 8]; // promised_stream_id + header fragment
    let buf = encode_frame(FrameType::PushPromise, 0, 1, &payload);
    let (frame, consumed) = Frame::decode(&buf).unwrap();
    assert_eq!(consumed, FRAME_HEADER_LEN);
    assert_eq!(
        frame,
        Frame::PushPromise {
            stream_id: 1,
            length: 8,
        }
    );
}

#[test]
fn incomplete_header_is_incomplete() {
    assert_eq!(Frame::decode(&[0u8; 4]), Err(FrameDecodeError::Incomplete));
}

#[test]
fn incomplete_control_payload_is_incomplete() {
    // PING frame declares length=8 but we only provide 4 payload bytes.
    let mut buf = vec![0u8; FRAME_HEADER_LEN + 4];
    FrameHeader {
        length: 8,
        frame_type: FrameType::Ping as u8,
        flags: 0,
        stream_id: 0,
    }
    .encode((&mut buf[..FRAME_HEADER_LEN]).try_into().unwrap());
    assert_eq!(Frame::decode(&buf), Err(FrameDecodeError::Incomplete));
}
