#![allow(clippy::cast_possible_truncation)] // fixed-size test payloads

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

// -- DATA frames --

#[test]
fn data_roundtrip_plain() {
    let payload = b"hello world";
    let prefix_len = data::encoded_prefix_len(0);
    let mut buf = vec![0u8; prefix_len + payload.len()];
    let written = data::encode_prefix(3, false, payload.len() as u32, 0, &mut buf).unwrap();
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
    let prefix_len = data::encoded_prefix_len(0);
    let mut buf = vec![0u8; prefix_len + payload.len()];
    data::encode_prefix(1, true, payload.len() as u32, 0, &mut buf).unwrap();
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
    let prefix_len = data::encoded_prefix_len(padding);
    let mut buf = vec![0u8; prefix_len + payload.len() + padding as usize];
    data::encode_prefix(5, false, payload.len() as u32, padding, &mut buf).unwrap();
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

// -- HEADERS frames --

#[test]
fn headers_roundtrip_plain() {
    let block = b"\x00\x00abc";
    let prefix_len = headers::encoded_prefix_len(0, false);
    let mut buf = vec![0u8; prefix_len + block.len()];
    headers::encode_prefix(7, false, true, None, block.len() as u32, 0, &mut buf).unwrap();
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
    let prefix_len = headers::encoded_prefix_len(padding, true);
    let mut buf = vec![0u8; prefix_len + block.len() + padding as usize];
    headers::encode_prefix(
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

// -- CONTINUATION frames --

#[test]
fn continuation_roundtrip() {
    let block = b"continued-fragment";
    let prefix_len = continuation::ENCODED_PREFIX_LEN;
    let mut buf = vec![0u8; prefix_len + block.len()];
    continuation::encode_prefix(9, true, block.len() as u32, &mut buf).unwrap();
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
