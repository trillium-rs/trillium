use super::*;

// -- FrameHeader tests --

/// Build wire bytes for a frame header from a raw type value and length.
fn encode_raw_header(frame_type: impl Into<u64>, payload_length: u64) -> Vec<u8> {
    let mut buf = vec![0; 256];
    let mut written = 0;
    written += quic_varint::encode(frame_type, &mut buf[written..]).unwrap();
    written += quic_varint::encode(payload_length, &mut buf[written..]).unwrap();
    buf.truncate(written);
    buf
}

#[test]
fn header_roundtrip_all_known_types() {
    for ft in [
        FrameType::Data,
        FrameType::Headers,
        FrameType::CancelPush,
        FrameType::Settings,
        FrameType::PushPromise,
        FrameType::Goaway,
        FrameType::MaxPushId,
    ] {
        let header = FrameHeader {
            frame_type: Some(ft),
            payload_length: 42,
        };
        let mut buf = vec![0; 256];

        let written = header.encode(&mut buf).unwrap();
        let (decoded, consumed) = FrameHeader::decode(&buf[..written]).unwrap();
        assert_eq!(decoded, header, "roundtrip failed for {ft:?}");
        assert_eq!(consumed, written);
    }
}

#[test]
fn header_unknown_frame_type() {
    let buf = encode_raw_header(0xFFu64, 100);
    let (header, consumed) = FrameHeader::decode(&buf).unwrap();
    assert_eq!(header.frame_type, None);
    assert_eq!(header.payload_length, 100);
    assert_eq!(consumed, buf.len());
}

#[test]
fn header_incomplete() {
    assert_eq!(FrameHeader::decode(&[]), Err(FrameDecodeError::Incomplete));
    // Type present but length missing
    let mut buf = vec![0; 256];
    let written = quic_varint::encode(FrameType::Headers, &mut buf).unwrap();
    assert_eq!(
        FrameHeader::decode(&buf[..written]),
        Err(FrameDecodeError::Incomplete)
    );
}

// -- Frame tests --

/// Build a complete wire frame from a raw type value and payload bytes.
fn encode_raw_frame(frame_type: impl Into<u64>, payload: &[u8]) -> Vec<u8> {
    let mut buf = vec![0; 256];
    let mut written = 0;

    written += quic_varint::encode(frame_type, &mut buf[written..]).unwrap();
    written += quic_varint::encode(payload.len() as u64, &mut buf[written..]).unwrap();
    buf.truncate(written);
    buf.extend_from_slice(payload);
    buf
}

#[test]
fn frame_data() {
    let header = encode_raw_header(FrameType::Data, 42);
    let header_len = header.len();
    let mut buf = header;
    buf.extend_from_slice(&[0xAA; 50]); // payload + trailing
    let (frame, consumed) = Frame::decode(&buf).unwrap();
    assert_eq!(frame, Frame::Data(42));
    assert_eq!(consumed, header_len); // payload not consumed
}

#[test]
fn frame_headers() {
    let buf = encode_raw_header(FrameType::Headers, 10);
    let (frame, consumed) = Frame::decode(&buf).unwrap();
    assert_eq!(frame, Frame::Headers(10));
    assert_eq!(consumed, buf.len());
}

#[test]
fn frame_unknown() {
    let buf = encode_raw_header(0xFFu64, 200);
    let (frame, consumed) = Frame::decode(&buf).unwrap();
    assert_eq!(frame, Frame::Unknown(200));
    assert_eq!(consumed, buf.len());
}

#[test]
fn frame_goaway() {
    let mut payload = vec![0; 256];
    let payload_len = quic_varint::encode(12u64, &mut payload).unwrap();
    let buf = encode_raw_frame(FrameType::Goaway, &payload[..payload_len]);
    let (frame, consumed) = Frame::decode(&buf).unwrap();
    assert_eq!(frame, Frame::Goaway(12));
    assert_eq!(consumed, buf.len());
}

#[test]
fn frame_cancel_push() {
    let mut payload = vec![0; 256];
    let payload_len = quic_varint::encode(7u64, &mut payload).unwrap();
    let buf = encode_raw_frame(FrameType::CancelPush, &payload[..payload_len]);
    let (frame, consumed) = Frame::decode(&buf).unwrap();
    assert_eq!(frame, Frame::CancelPush(7));
    assert_eq!(consumed, buf.len());
}

#[test]
fn frame_max_push_id() {
    let mut payload = vec![0; 256];
    let payload_len = quic_varint::encode(99u64, &mut payload).unwrap();
    let buf = encode_raw_frame(FrameType::MaxPushId, &payload[..payload_len]);
    let (frame, consumed) = Frame::decode(&buf).unwrap();
    assert_eq!(frame, Frame::MaxPushId(99));
    assert_eq!(consumed, buf.len());
}

#[test]
fn frame_settings_roundtrip() {
    let settings = H3Settings::new().with_max_field_section_size(8192);
    let mut payload = vec![0; 256];
    let payload_len = settings.encode(&mut payload).unwrap();
    let buf = encode_raw_frame(FrameType::Settings, &payload[..payload_len]);
    let (frame, consumed) = Frame::decode(&buf).unwrap();
    assert_eq!(frame, Frame::Settings(settings));
    assert_eq!(consumed, buf.len());
}

#[test]
fn frame_goaway_trailing_bytes_is_error() {
    // Goaway payload should be exactly one varint. Add an extra byte.
    let mut payload = Vec::new();
    quic_varint::encode(5u64, &mut payload);
    payload.push(0xFF); // trailing garbage
    let buf = encode_raw_frame(FrameType::Goaway, &payload);
    assert_eq!(
        Frame::decode(&buf),
        Err(FrameDecodeError::Error(H3ErrorCode::FrameError))
    );
}

#[test]
fn frame_push_promise() {
    // PushPromise payload: varint(push_id) + field section bytes
    let mut payload = vec![0; 256];
    let push_id_len = quic_varint::encode(3u64, &mut payload).unwrap(); // push_id = 3
    payload.truncate(push_id_len);
    payload.extend_from_slice(b"fake qpack field section");
    let field_section_length = payload.len() - push_id_len;

    let mut buf = encode_raw_frame(FrameType::PushPromise, &payload);
    buf.extend_from_slice(b"trailing");
    let (frame, consumed) = Frame::decode(&buf).unwrap();
    assert_eq!(
        frame,
        Frame::PushPromise {
            push_id: 3,
            field_section_length: field_section_length as u64,
        }
    );
    // consumed includes frame header + push_id varint; field section + trailing remain
    let rest = &buf[consumed..];
    assert!(rest.starts_with(b"fake qpack field section"));
    assert!(rest.ends_with(b"trailing"));
}

#[test]
fn frame_push_promise_incomplete() {
    // Frame header present but push_id varint not available
    let buf = encode_raw_header(FrameType::PushPromise, 100);
    // no payload bytes at all
    assert_eq!(Frame::decode(&buf), Err(FrameDecodeError::Incomplete));
}

#[test]
fn frame_incomplete_control() {
    // Settings frame header present but payload not fully available
    let mut buf = encode_raw_header(FrameType::Settings, 20);
    buf.extend_from_slice(&[0; 5]); // only 5 of 20 payload bytes
    assert_eq!(Frame::decode(&buf), Err(FrameDecodeError::Incomplete));
}

#[test]
fn frame_incomplete_empty() {
    assert_eq!(Frame::decode(&[]), Err(FrameDecodeError::Incomplete));
}

// -- Frame encode tests --

#[test]
fn encode_decode_roundtrip_data() {
    let frame = Frame::Data(1024);
    let mut buf = [0u8; 16];
    let n = frame.encode(&mut buf).unwrap();
    assert_eq!(n, frame.encoded_len());
    let (decoded, consumed) = Frame::decode(&buf[..n]).unwrap();
    assert_eq!(decoded, frame);
    assert_eq!(consumed, n);
}

#[test]
fn encode_decode_roundtrip_goaway() {
    let frame = Frame::Goaway(42);
    let mut buf = [0u8; 16];
    let n = frame.encode(&mut buf).unwrap();
    assert_eq!(n, frame.encoded_len());
    let (decoded, consumed) = Frame::decode(&buf[..n]).unwrap();
    assert_eq!(decoded, frame);
    assert_eq!(consumed, n);
}

#[test]
fn encode_decode_roundtrip_settings() {
    let settings = H3Settings::new()
        .with_max_field_section_size(8192)
        .with_qpack_max_table_capacity(4096);
    let frame = Frame::Settings(settings);
    let mut buf = [0u8; 64];
    let n = frame.encode(&mut buf).unwrap();
    assert_eq!(n, frame.encoded_len());
    let (decoded, consumed) = Frame::decode(&buf[..n]).unwrap();
    assert_eq!(decoded, frame);
    assert_eq!(consumed, n);
}

#[test]
fn encode_decode_roundtrip_push_promise() {
    let frame = Frame::PushPromise {
        push_id: 7,
        field_section_length: 100,
    };
    let mut buf = [0u8; 16];
    let n = frame.encode(&mut buf).unwrap();
    assert_eq!(n, frame.encoded_len());
    let (decoded, consumed) = Frame::decode(&buf[..n]).unwrap();
    assert_eq!(decoded, frame);
    assert_eq!(consumed, n);
}

#[test]
fn encode_buffer_too_small() {
    let frame = Frame::Goaway(42);
    let mut buf = [0u8; 1]; // too small
    assert!(frame.encode(&mut buf).is_none());
}

#[test]
fn encode_unknown_is_noop() {
    let frame = Frame::Unknown(500);
    let mut buf = [0u8; 16];
    assert_eq!(frame.encode(&mut buf), Some(0));
    assert_eq!(frame.encoded_len(), 0);
}
