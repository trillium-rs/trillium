//! Unit tests for [`H3Frame::decode`] — the synchronous frame-boundary state machine,
//! driven directly with a single input buffer.

use super::*;
use trillium_testing::test;

/// Helper to call `h3_frame_decode` and return (state, `output_bytes`).
fn decode(
    remaining_in_frame: u64,
    total: u64,
    frame_type: H3BodyFrameType,
    input: &[u8],
    content_length: Option<u64>,
) -> io::Result<(ReceivedBodyState, Vec<u8>)> {
    decode_with_max_len(
        remaining_in_frame,
        total,
        frame_type,
        input,
        content_length,
        1024 * 1024,
    )
}

fn decode_with_max_len(
    remaining_in_frame: u64,
    total: u64,
    frame_type: H3BodyFrameType,
    input: &[u8],
    content_length: Option<u64>,
    max_len: u64,
) -> io::Result<(ReceivedBodyState, Vec<u8>)> {
    decode_with_limits(
        remaining_in_frame,
        total,
        frame_type,
        input,
        content_length,
        max_len,
        u64::MAX,
    )
}

fn decode_with_limits(
    remaining_in_frame: u64,
    total: u64,
    frame_type: H3BodyFrameType,
    input: &[u8],
    content_length: Option<u64>,
    max_len: u64,
    max_trailer_size: u64,
) -> io::Result<(ReceivedBodyState, Vec<u8>)> {
    let mut buf = input.to_vec();
    let mut self_buffer = Buffer::default();
    let mut trailer_payload_buffer = Vec::new();
    let h3 = H3Connection::new(Default::default());
    let stream_id = 1;
    let (state, bytes) = H3Frame {
        self_buffer: &mut self_buffer,
        trailer_payload_buffer: &mut trailer_payload_buffer,
        remaining_in_frame,
        total,
        frame_type,
        buf: &mut buf,
        content_length,
        max_len,
        max_trailer_size,
        connection: Some((&h3, stream_id)),
        trailers_future: &mut None,
    }
    .decode()?;
    Ok((state, buf[..bytes].to_vec()))
}

#[test]
fn single_data_frame() {
    let input = data_frame(b"hello");
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"hello");
    assert_eq!(
        state,
        ReceivedBodyState::H3Data {
            remaining_in_frame: 0,
            total: 5,
            frame_type: H3BodyFrameType::Data,
            partial_frame_header: false,
        }
    );
}

#[test]
fn two_data_frames() {
    let mut input = data_frame(b"hello");
    input.extend_from_slice(&data_frame(b" world"));
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"hello world");
    assert_eq!(
        state,
        ReceivedBodyState::H3Data {
            remaining_in_frame: 0,
            total: 11,
            frame_type: H3BodyFrameType::Data,
            partial_frame_header: false,
        }
    );
}

#[test]
fn mid_frame_entry() {
    // Simulate entering with 5 bytes remaining in a DATA frame
    let (state, body) = decode(5, 0, H3BodyFrameType::Data, b"hello", None).unwrap();
    assert_eq!(body, b"hello");
    assert_eq!(
        state,
        ReceivedBodyState::H3Data {
            remaining_in_frame: 0,
            total: 5,
            frame_type: H3BodyFrameType::Data,
            partial_frame_header: false,
        }
    );
}

#[test]
fn mid_frame_then_next_frame() {
    // 3 bytes remaining in current frame, then a new DATA frame follows
    let mut input = b"abc".to_vec();
    input.extend_from_slice(&data_frame(b"def"));
    let (state, body) = decode(3, 0, H3BodyFrameType::Data, &input, None).unwrap();
    assert_eq!(body, b"abcdef");
    assert_eq!(
        state,
        ReceivedBodyState::H3Data {
            remaining_in_frame: 0,
            total: 6,
            frame_type: H3BodyFrameType::Data,
            partial_frame_header: false,
        }
    );
}

#[test]
fn partial_frame_at_end() {
    // DATA frame followed by an incomplete frame header (just the type byte)
    let mut input = data_frame(b"hello");
    input.push(0x00); // start of another DATA frame header, but no length
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"hello");
    assert!(matches!(
        state,
        ReceivedBodyState::H3Data {
            partial_frame_header: true,
            ..
        }
    ));
}

#[test]
fn content_length_match() {
    let input = data_frame(b"hello");
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, Some(5)).unwrap();
    assert_eq!(body, b"hello");
    assert!(matches!(state, ReceivedBodyState::H3Data { total: 5, .. }));
}

#[test]
fn content_length_exceeded() {
    let input = data_frame(b"hello world");
    let err = decode(0, 0, H3BodyFrameType::Start, &input, Some(5)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

#[test]
fn max_len_exceeded() {
    let input = data_frame(b"hello");
    let err = decode_with_max_len(0, 0, H3BodyFrameType::Start, &input, None, 3).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Unsupported);
}

#[test]
fn unknown_frame_skipped() {
    let mut input = unknown_frame(0x21, b"xxx");
    input.extend_from_slice(&data_frame(b"hello"));
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"hello");
    assert!(matches!(state, ReceivedBodyState::H3Data { total: 5, .. }));
}

#[test]
fn unexpected_frame_type_is_error() {
    // SETTINGS frame (type 0x04) on a request stream
    let input = vec![0x04, 0x00];
    let err = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

#[test]
fn empty_data_frame() {
    let input = data_frame(b"");
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"");
    assert_eq!(
        state,
        ReceivedBodyState::H3Data {
            remaining_in_frame: 0,
            total: 0,
            frame_type: H3BodyFrameType::Data,
            partial_frame_header: false,
        }
    );
}

#[test]
fn empty_data_frame_then_data() {
    let mut input = data_frame(b"");
    input.extend_from_slice(&data_frame(b"hello"));
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"hello");
    assert!(matches!(state, ReceivedBodyState::H3Data { total: 5, .. }));
}

#[test]
fn data_frame_larger_than_buffer() {
    // Simulate a DATA frame with 100 bytes, but we only have 10 bytes of payload
    let (state, body) = decode(100, 0, H3BodyFrameType::Data, b"0123456789", None).unwrap();
    assert_eq!(body, b"0123456789");
    assert_eq!(
        state,
        ReceivedBodyState::H3Data {
            remaining_in_frame: 90,
            total: 10,
            frame_type: H3BodyFrameType::Data,
            partial_frame_header: false,
        }
    );
}

#[test]
fn unknown_frame_before_data() {
    let mut input = unknown_frame(0x21, b"skip me");
    input.extend_from_slice(&data_frame(b"body"));
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"body");
    assert!(matches!(state, ReceivedBodyState::H3Data { total: 4, .. }));
}

#[test]
fn multiple_unknown_frames_interspersed() {
    let mut input = data_frame(b"aaa");
    input.extend_from_slice(&unknown_frame(0x21, b"x"));
    input.extend_from_slice(&data_frame(b"bbb"));
    input.extend_from_slice(&unknown_frame(0x22, b"yy"));
    input.extend_from_slice(&data_frame(b"ccc"));
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"aaabbbccc");
    assert!(matches!(state, ReceivedBodyState::H3Data { total: 9, .. }));
}

#[test]
fn zero_length_unknown_frame() {
    let mut input = unknown_frame(0x21, b"");
    input.extend_from_slice(&data_frame(b"hello"));
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"hello");
    assert!(matches!(state, ReceivedBodyState::H3Data { total: 5, .. }));
}

#[test]
fn trailers_end_body() {
    let mut input = data_frame(b"body");
    let (_sent_trailers, trailers_buf) = build_trailers();
    input.extend_from_slice(&trailers_buf);
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"body");
    assert_eq!(state, End);
}

#[test]
fn trailers_with_content_length_match() {
    let mut input = data_frame(b"body");
    let (_sent_trailers, trailers_buf) = build_trailers();
    input.extend_from_slice(&trailers_buf);
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, Some(4)).unwrap();
    assert_eq!(body, b"body");
    assert_eq!(state, End);
}

#[test]
fn trailers_with_content_length_mismatch() {
    let mut input = data_frame(b"body");
    input.extend_from_slice(&headers_frame(5));
    let (_sent_trailers, trailers_buf) = build_trailers();
    input.extend_from_slice(&trailers_buf);
    let err = decode(0, 0, H3BodyFrameType::Start, &input, Some(10)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

#[test]
fn trailers_exceed_max_trailer_size() {
    // Trailer HEADERS frame declaring 100 bytes of payload, limit is 50 bytes.
    let mut input = data_frame(b"body");
    let (_sent_trailers, trailers_buf) = build_trailers();
    input.extend_from_slice(&trailers_buf);
    let err = decode_with_limits(0, 0, H3BodyFrameType::Start, &input, None, 1024 * 1024, 10)
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Other);
    // Verify we embedded the right H3 error code for the QUIC layer to surface.
    let h3_code = err
        .get_ref()
        .and_then(|e| e.downcast_ref::<H3ErrorCode>())
        .copied();
    assert_eq!(h3_code, Some(H3ErrorCode::MessageError));
}

fn build_trailers() -> (Headers, Vec<u8>) {
    let mut trailers = Headers::new();
    trailers.insert(KnownHeaderName::Trailer, "x-checksum");
    trailers.insert("x-checksum", "abc123");

    let mut qpack_buf = Vec::new();
    H3Connection::new(Default::default())
        .encode_field_section(
            &FieldSection::new(PseudoHeaders::default(), &trailers),
            &mut qpack_buf,
            1,
        )
        .unwrap();
    let mut input = vec![];
    input.extend_from_slice(&headers_frame(qpack_buf.len() as u64));
    input.extend_from_slice(&qpack_buf);
    (trailers, input)
}

#[test]
fn trailers_decoded_into_destination() {
    let mut input = data_frame(b"body");
    let (sent_trailers, trailers_buf) = build_trailers();
    input.extend_from_slice(&trailers_buf);
    let mut buf = input.clone();
    let mut self_buffer = Buffer::default();
    let mut trailer_payload_buffer = Vec::new();
    let mut trailers_fut = None;
    let stream_id = 1;
    let h3 = H3Connection::new(Default::default());

    let (state, bytes) = H3Frame {
        self_buffer: &mut self_buffer,
        trailer_payload_buffer: &mut trailer_payload_buffer,
        remaining_in_frame: 0,
        total: 0,
        frame_type: H3BodyFrameType::Start,
        buf: &mut buf,
        content_length: None,
        max_len: 1024 * 1024,
        max_trailer_size: u64::MAX,
        connection: Some((&h3, stream_id)),
        trailers_future: &mut trailers_fut,
    }
    .decode()
    .unwrap();

    assert_eq!(&buf[..bytes], b"body");
    assert_eq!(state, End);
    let received_trailers = trillium_testing::block_on(trailers_fut.unwrap()).unwrap();
    assert_eq!(received_trailers.get_str("x-checksum"), Some("abc123"));
    assert_eq!(sent_trailers, received_trailers);
}

#[test]
fn trailers_at_max_trailer_size_allowed() {
    // Exactly at the limit must succeed.
    let mut input = data_frame(b"body");

    let mut trailers = Headers::new();
    trailers.insert(KnownHeaderName::Trailer, "x-checksum");
    trailers.insert("x-checksum", "abc123");

    let mut qpack_buf = Vec::new();
    H3Connection::new(Default::default())
        .encode_field_section(
            &FieldSection::new(PseudoHeaders::default(), &trailers),
            &mut qpack_buf,
            1,
        )
        .unwrap();
    let payload_len = qpack_buf.len() as u64;
    input.extend_from_slice(&headers_frame(payload_len));
    input.extend_from_slice(&qpack_buf);

    let (state, body) = decode_with_limits(
        0,
        0,
        H3BodyFrameType::Start,
        &input,
        None,
        1024 * 1024,
        payload_len,
    )
    .unwrap();
    assert_eq!(body, b"body");
    assert_eq!(state, End);
}

#[test]
fn unknown_frame_larger_than_buffer() {
    // Unknown frame with 20 bytes payload, but only 5 bytes of it are in this buffer
    let (state, body) = decode(20, 0, H3BodyFrameType::Unknown, b"12345", None).unwrap();
    assert_eq!(body, b"");
    assert_eq!(
        state,
        ReceivedBodyState::H3Data {
            remaining_in_frame: 15,
            total: 0,
            frame_type: H3BodyFrameType::Unknown,
            partial_frame_header: false,
        }
    );
}

#[test]
fn mid_unknown_then_data() {
    // 3 bytes remaining in unknown frame, then a DATA frame
    let mut input = b"xxx".to_vec();
    input.extend_from_slice(&data_frame(b"real"));
    let (state, body) = decode(3, 0, H3BodyFrameType::Unknown, &input, None).unwrap();
    assert_eq!(body, b"real");
    assert!(matches!(state, ReceivedBodyState::H3Data { total: 4, .. }));
}

#[test]
fn content_length_exceeded_across_frames() {
    // Two DATA frames that together exceed content-length
    let mut input = data_frame(b"abc");
    input.extend_from_slice(&data_frame(b"def"));
    let err = decode(0, 0, H3BodyFrameType::Start, &input, Some(5)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
}

#[test]
fn max_len_exceeded_mid_frame() {
    // Enter mid-frame with total already near the limit
    let err =
        decode_with_max_len(10, 95, H3BodyFrameType::Data, b"0123456789", None, 100).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Unsupported);
}

// --- GREASE frame tests (8-byte varint frame types, like curl sends) ---

#[test]
fn grease_frame_skipped() {
    let mut input = grease_frame(1_000_000, b"GREASE is the word");
    input.extend_from_slice(&data_frame(b"hello"));
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"hello");
    assert!(matches!(state, ReceivedBodyState::H3Data { total: 5, .. }));
}

#[test]
fn grease_frame_between_data_frames() {
    let mut input = data_frame(b"aaa");
    input.extend_from_slice(&grease_frame(999_999, b"grease payload"));
    input.extend_from_slice(&data_frame(b"bbb"));
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"aaabbb");
    assert!(matches!(state, ReceivedBodyState::H3Data { total: 6, .. }));
}

#[test]
fn grease_frame_spans_buffer_boundary() {
    // GREASE frame with 20 bytes payload, but only 5 bytes available
    let grease = grease_frame(500_000, &[0xAA; 20]);
    // Take just the header + 5 bytes of payload
    let header_end = grease.len() - 20;
    let input = grease[..header_end + 5].to_vec();
    // This should leave us mid-unknown-frame
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"");
    assert_eq!(
        state,
        ReceivedBodyState::H3Data {
            remaining_in_frame: 15,
            total: 0,
            frame_type: H3BodyFrameType::Unknown,
            partial_frame_header: false,
        }
    );

    // Now continue with remaining 15 bytes of GREASE payload + a DATA frame
    let mut input2 = vec![0xAA; 15];
    input2.extend_from_slice(&data_frame(b"after grease"));
    let (state, body) = decode(15, 0, H3BodyFrameType::Unknown, &input2, None).unwrap();
    assert_eq!(body, b"after grease");
    assert!(matches!(state, ReceivedBodyState::H3Data { total: 12, .. }));
}
