use super::*;
use crate::{HttpConfig, h3::Frame, http_config::DEFAULT_CONFIG};
use encoding_rs::UTF_8;
use futures_lite::{AsyncRead, AsyncReadExt, io::Cursor};
use test_harness::test;
use trillium_testing::harness;

/// Encode a DATA frame (header + payload) into a Vec.
fn data_frame(payload: &[u8]) -> Vec<u8> {
    let frame = Frame::Data(payload.len() as u64);
    let header_len = frame.encoded_len();
    let mut buf = vec![0u8; header_len + payload.len()];
    frame.encode(&mut buf).unwrap();
    buf[header_len..].copy_from_slice(payload);
    buf
}

/// Encode a HEADERS frame header (no payload — caller appends QPACK bytes).
fn headers_frame(payload_len: u64) -> Vec<u8> {
    let frame = Frame::Headers(payload_len);
    let header_len = frame.encoded_len();
    let mut buf = vec![0u8; header_len];
    frame.encode(&mut buf).unwrap();
    buf
}

/// Encode an unknown frame (type not in the H3 spec) with the given payload.
fn unknown_frame(type_value: u8, payload: &[u8]) -> Vec<u8> {
    let mut buf = vec![];
    buf.push(type_value); // 1-byte QUIC varint for type (must be ≤ 0x3F)
    buf.push(payload.len() as u8); // 1-byte QUIC varint for length (must be ≤ 0x3F)
    buf.extend_from_slice(payload);
    buf
}

/// Encode a QUIC varint, appending to `out`.
fn encode_varint(value: u64, out: &mut Vec<u8>) {
    if value < (1 << 6) {
        out.push(value as u8);
    } else if value < (1 << 14) {
        out.push(0x40 | (value >> 8) as u8);
        out.push(value as u8);
    } else if value < (1 << 30) {
        out.extend_from_slice(&[
            0x80 | (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ]);
    } else {
        out.extend_from_slice(&[
            0xC0 | (value >> 56) as u8,
            (value >> 48) as u8,
            (value >> 40) as u8,
            (value >> 32) as u8,
            (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ]);
    }
}

/// Encode a GREASE frame with a large (8-byte varint) type value.
/// GREASE type values are of the form `0x1f * N + 0x21`.
fn grease_frame(n: u64, payload: &[u8]) -> Vec<u8> {
    let grease_type = 0x1f * n + 0x21;
    let mut buf = vec![];
    encode_varint(grease_type, &mut buf);
    encode_varint(payload.len() as u64, &mut buf);
    buf.extend_from_slice(payload);
    buf
}

/// Helper to call h3_frame_decode and return (state, output_bytes).
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
    let mut buf = input.to_vec();
    let mut self_buffer = Buffer::default();
    let (state, bytes) = h3_frame_decode(
        &mut self_buffer,
        remaining_in_frame,
        total,
        frame_type,
        &mut buf,
        content_length,
        max_len,
    )?;
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
    input.extend_from_slice(&headers_frame(5));
    input.extend_from_slice(b"trail"); // fake QPACK payload
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, None).unwrap();
    assert_eq!(body, b"body");
    assert_eq!(state, End);
}

#[test]
fn trailers_with_content_length_match() {
    let mut input = data_frame(b"body");
    input.extend_from_slice(&headers_frame(5));
    input.extend_from_slice(b"trail");
    let (state, body) = decode(0, 0, H3BodyFrameType::Start, &input, Some(4)).unwrap();
    assert_eq!(body, b"body");
    assert_eq!(state, End);
}

#[test]
fn trailers_with_content_length_mismatch() {
    let mut input = data_frame(b"body");
    input.extend_from_slice(&headers_frame(5));
    input.extend_from_slice(b"trail");
    let err = decode(0, 0, H3BodyFrameType::Start, &input, Some(10)).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidData);
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

// --- Async tests using ReceivedBody ---

async fn read_with_buffers_of_size<R>(reader: &mut R, size: usize) -> crate::Result<String>
where
    R: AsyncRead + Unpin,
{
    let mut return_buffer = vec![];
    loop {
        let mut buf = vec![0; size];
        match reader.read(&mut buf).await? {
            0 => break Ok(String::from_utf8_lossy(&return_buffer).into()),
            bytes_read => return_buffer.extend_from_slice(&buf[..bytes_read]),
        }
    }
}

fn new_h3_body(
    input: Vec<u8>,
    content_length: Option<u64>,
    config: &HttpConfig,
) -> ReceivedBody<'_, Cursor<Vec<u8>>> {
    ReceivedBody::new_with_config(
        content_length,
        Buffer::from(Vec::with_capacity(config.response_header_initial_capacity)),
        Cursor::new(input),
        ReceivedBodyState::H3Data {
            remaining_in_frame: 0,
            total: 0,
            frame_type: H3BodyFrameType::Start,
            partial_frame_header: false,
        },
        None,
        UTF_8,
        config,
    )
}

/// Build DATA-framed bytes from a raw body string.
fn frame_body(body: &str) -> Vec<u8> {
    data_frame(body.as_bytes())
}

/// Build DATA-framed bytes as multiple small frames of `chunk_size`.
fn frame_body_chunked(body: &str, chunk_size: usize) -> Vec<u8> {
    let mut out = vec![];
    for chunk in body.as_bytes().chunks(chunk_size) {
        out.extend_from_slice(&data_frame(chunk));
    }
    out
}

async fn h3_decode(input: Vec<u8>, poll_size: usize) -> crate::Result<String> {
    let mut rb = new_h3_body(input, None, &DEFAULT_CONFIG);
    read_with_buffers_of_size(&mut rb, poll_size).await
}

#[test(harness)]
async fn async_single_frame_various_buffer_sizes() {
    let body = "hello world";
    let framed = frame_body(body);
    for size in 1..50 {
        let output = h3_decode(framed.clone(), size).await.unwrap();
        assert_eq!(output, body, "size: {size}");
    }
}

#[test(harness)]
async fn async_multiple_frames_various_buffer_sizes() {
    let body = "the quick brown fox jumps over the lazy dog";
    let framed = frame_body_chunked(body, 5);
    for size in 1..50 {
        let output = h3_decode(framed.clone(), size).await.unwrap();
        assert_eq!(output, body, "size: {size}");
    }
}

#[test(harness)]
async fn async_with_unknown_frames_interspersed() {
    let mut framed = vec![];
    framed.extend_from_slice(&data_frame(b"hello"));
    framed.extend_from_slice(&unknown_frame(0x21, b"skip"));
    framed.extend_from_slice(&data_frame(b" "));
    framed.extend_from_slice(&unknown_frame(0x22, b""));
    framed.extend_from_slice(&data_frame(b"world"));
    for size in 1..50 {
        let output = h3_decode(framed.clone(), size).await.unwrap();
        assert_eq!(output, "hello world", "size: {size}");
    }
}

#[test(harness)]
async fn async_content_length_valid() {
    let body = "test ".repeat(50);
    let framed = frame_body(&body);
    let rb = new_h3_body(framed, Some(body.len() as u64), &DEFAULT_CONFIG);
    let output = rb.read_string().await.unwrap();
    assert_eq!(output, body);
}

#[test(harness)]
async fn async_content_length_mismatch() {
    let body = "test ".repeat(50);
    let framed = frame_body(&body);
    // Claim content-length is shorter than actual
    let rb = new_h3_body(framed, Some(10), &DEFAULT_CONFIG);
    assert!(rb.read_string().await.is_err());
}

#[test(harness)]
async fn async_max_len() {
    let body = "test ".repeat(100);
    let framed = frame_body(&body);

    // Should succeed with default max_len
    let rb = new_h3_body(framed.clone(), None, &DEFAULT_CONFIG);
    assert!(rb.read_string().await.is_ok());

    // Should fail with small max_len
    let config = DEFAULT_CONFIG.with_received_body_max_len(100);
    let rb = new_h3_body(framed, None, &config);
    assert!(rb.read_string().await.is_err());
}

#[test(harness)]
async fn async_empty_body() {
    // No DATA frames at all — just an empty stream
    let framed = vec![];
    let rb = new_h3_body(framed, None, &DEFAULT_CONFIG);
    let output = rb.read_string().await.unwrap();
    assert_eq!(output, "");
}

#[test(harness)]
async fn async_empty_data_frame() {
    let framed = data_frame(b"");
    for size in 1..20 {
        let output = h3_decode(framed.clone(), size).await.unwrap();
        assert_eq!(output, "", "size: {size}");
    }
}

#[test(harness)]
async fn async_large_body_various_frame_and_buffer_sizes() {
    let body = "abcdefghij".repeat(100); // 1000 bytes
    for chunk_size in [1, 7, 50, 100, 999, 1000] {
        let framed = frame_body_chunked(&body, chunk_size);
        for buf_size in [3, 10, 64, 256, 1024] {
            let output = h3_decode(framed.clone(), buf_size).await.unwrap();
            assert_eq!(
                output, body,
                "chunk_size: {chunk_size}, buf_size: {buf_size}"
            );
        }
    }
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

#[test(harness)]
async fn async_grease_interspersed_various_buffer_sizes() {
    let mut framed = vec![];
    framed.extend_from_slice(&grease_frame(1_000_000, b"GREASE is the word"));
    framed.extend_from_slice(&data_frame(b"hello"));
    framed.extend_from_slice(&grease_frame(2_000_000, b""));
    framed.extend_from_slice(&data_frame(b" "));
    framed.extend_from_slice(&grease_frame(3_000_000, b"more grease"));
    framed.extend_from_slice(&data_frame(b"world"));

    for size in 1..60 {
        let output = h3_decode(framed.clone(), size).await.unwrap();
        assert_eq!(output, "hello world", "buf_size: {size}");
    }
}

#[test(harness)]
async fn async_grease_only_buffer() {
    // A buffer where the entire read is GREASE — no DATA bytes at all in
    // the first several reads, then DATA follows.
    let mut framed = vec![];
    // Several GREASE frames totaling ~100 bytes
    for i in 0..5 {
        framed.extend_from_slice(&grease_frame(1_000_000 + i, b"grease padding!"));
    }
    framed.extend_from_slice(&data_frame(b"finally data"));

    for size in [1, 3, 10, 16, 32, 64, 128] {
        let output = h3_decode(framed.clone(), size).await.unwrap();
        assert_eq!(output, "finally data", "buf_size: {size}");
    }
}
