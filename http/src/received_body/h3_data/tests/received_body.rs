//! Tests that drive a whole [`ReceivedBody`] over a transport, exercising the poll loop
//! across buffer boundaries and read sizes.

use super::*;
use trillium_testing::{harness, test};

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
    let mut rb = new_h3_body(input, None, &HttpConfig::DEFAULT);
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
    let rb = new_h3_body(framed, Some(body.len() as u64), &HttpConfig::DEFAULT);
    let output = rb.read_string().await.unwrap();
    assert_eq!(output, body);
}

#[test(harness)]
async fn async_content_length_mismatch() {
    let body = "test ".repeat(50);
    let framed = frame_body(&body);
    // Claim content-length is shorter than actual
    let rb = new_h3_body(framed, Some(10), &HttpConfig::DEFAULT);
    assert!(rb.read_string().await.is_err());
}

#[test(harness)]
async fn async_max_len() {
    let body = "test ".repeat(100);
    let framed = frame_body(&body);

    // Should succeed with default max_len
    let rb = new_h3_body(framed.clone(), None, &HttpConfig::DEFAULT);
    assert!(rb.read_string().await.is_ok());

    // Should fail with small max_len
    let config = HttpConfig::DEFAULT.with_received_body_max_len(100);
    let rb = new_h3_body(framed, None, &config);
    assert!(rb.read_string().await.is_err());
}

#[test(harness)]
async fn async_empty_body() {
    // No DATA frames at all — just an empty stream
    let framed = vec![];
    let rb = new_h3_body(framed, None, &HttpConfig::DEFAULT);
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

/// Like [`new_h3_body`] but the framed bytes start *pre-buffered* (as if over-read into the
/// conn buffer alongside the response headers) with an empty transport — nothing left on the
/// wire. The body must drain from the buffer without depending on the transport.
fn new_h3_body_prebuffered(input: Vec<u8>) -> ReceivedBody<'static, Cursor<Vec<u8>>> {
    ReceivedBody::new_with_config(
        None,
        Buffer::from(input),
        Cursor::new(Vec::new()),
        ReceivedBodyState::new_h3(),
        None,
        UTF_8,
        &HttpConfig::DEFAULT,
    )
}

#[test(harness)]
async fn buffered_h3_body_drains_without_transport() {
    // Regression: a DATA frame fully buffered (e.g. read alongside the response headers)
    // with an idle/closed transport must still be readable, including with a read buffer
    // smaller than the frame header. The `partial_frame_header` recovery previously read the
    // transport instead of draining `self.buffer`, hanging an open idle stream / erroring a
    // closed one. The 200-byte payload gives a 3-byte DATA header, so 1- and 2-byte reads
    // both land in the partial path.
    let payload = "abcdefghij".repeat(20);
    for size in [1usize, 2, 3, 7, 64, 4096] {
        let mut rb = new_h3_body_prebuffered(data_frame(payload.as_bytes()));
        let output = read_with_buffers_of_size(&mut rb, size).await.unwrap();
        assert_eq!(output, payload, "buf_size: {size}");
    }
}

#[test(harness)]
async fn truncated_frame_payload_is_not_clean_eof() {
    // Each frame header promises 63 payload bytes, but only 3 arrive before EOF. The
    // content-length values are the ones a truncation-blind reader would accept as a
    // complete body.
    for (frame_type, content_length) in [
        (0x00, None),
        (0x00, Some(3)),
        (0x01, None),
        (0x01, Some(0)),
        (0x21, None),
        (0x21, Some(0)),
    ] {
        let mut body = new_h3_body(
            vec![frame_type, 63, b'a', b'b', b'c'],
            content_length,
            &HttpConfig::DEFAULT,
        );
        let result = read_with_buffers_of_size(&mut body, 16).await;
        assert!(
            result.is_err(),
            "frame type {frame_type:#x}, content-length {content_length:?}: {result:?}"
        );
    }
}

#[test(harness)]
async fn truncated_frame_header_is_not_clean_eof() {
    for input in [vec![0x00], vec![0x40], vec![0x00, 0x40]] {
        let mut body = new_h3_body(input.clone(), None, &HttpConfig::DEFAULT);
        let result = read_with_buffers_of_size(&mut body, 16).await;
        assert!(result.is_err(), "{input:?}: {result:?}");
    }
}
