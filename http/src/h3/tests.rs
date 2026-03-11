use crate::{
    Buffer,
    body::BodyType,
    h3::H3Body,
    http_config::DEFAULT_CONFIG,
    received_body::{H3BodyFrameType, ReceivedBody, ReceivedBodyState},
};
use encoding_rs::UTF_8;
use futures_lite::{AsyncReadExt, io::Cursor};
use std::net::Shutdown;
use test_harness::test;
use trillium_testing::{TestTransport, harness};

/// Encode a body through `H3BodyWrapper` and decode it back through `ReceivedBody`,
/// running both sides concurrently through a `TestTransport` pair.
async fn round_trip(body: BodyType, content_length: Option<u64>) -> String {
    let (mut writer, reader) = TestTransport::new();

    let rb = ReceivedBody::new_with_config(
        content_length,
        Buffer::from(Vec::with_capacity(
            DEFAULT_CONFIG.response_header_initial_capacity,
        )),
        reader,
        ReceivedBodyState::new_h3(),
        None,
        UTF_8,
        &DEFAULT_CONFIG,
    );

    let (_, result) = futures_lite::future::zip(
        async {
            futures_lite::io::copy(H3Body::from(body), &mut writer)
                .await
                .unwrap();
            writer.shutdown(Shutdown::Write);
        },
        rb.read_string(),
    )
    .await;

    result.unwrap()
}

/// Like `round_trip` but drives `H3BodyWrapper` with a fixed-size poll buffer, producing
/// multiple DATA frames when `buf_size` is smaller than the body.
async fn round_trip_buf(body: BodyType, content_length: Option<u64>, buf_size: usize) -> String {
    let (writer, reader) = TestTransport::new();

    let rb = ReceivedBody::new_with_config(
        content_length,
        Buffer::from(Vec::with_capacity(
            DEFAULT_CONFIG.response_header_initial_capacity,
        )),
        reader,
        ReceivedBodyState::H3Data {
            remaining_in_frame: 0,
            total: 0,
            frame_type: H3BodyFrameType::Start,
            partial_frame_header: false,
        },
        None,
        UTF_8,
        &DEFAULT_CONFIG,
    );

    let (_, result) = futures_lite::future::zip(
        async {
            let mut src = H3Body::from(body);
            let mut buf = vec![0u8; buf_size];
            loop {
                let n = src.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                writer.write_all(&buf[..n]);
            }
            writer.shutdown(Shutdown::Write);
        },
        rb.read_string(),
    )
    .await;

    result.unwrap()
}

#[test(harness)]
async fn empty_body() {
    assert_eq!(round_trip(BodyType::Empty, None).await, "");
}

#[test(harness)]
async fn static_body() {
    let body = "hello world";
    let result = round_trip(
        BodyType::Static {
            content: body.as_bytes().into(),
            cursor: 0,
        },
        Some(body.len() as u64),
    )
    .await;
    assert_eq!(result, body);
}

#[test(harness)]
async fn streaming_known_length() {
    let body = "hello streaming world";
    let result = round_trip(
        BodyType::Streaming {
            async_read: Box::pin(Cursor::new(body.as_bytes().to_vec())),
            len: Some(body.len() as u64),
            done: false,
            progress: 0,
        },
        Some(body.len() as u64),
    )
    .await;
    assert_eq!(result, body);
}

#[test(harness)]
async fn streaming_unknown_length() {
    let body = "hello chunked world";
    let result = round_trip(
        BodyType::Streaming {
            async_read: Box::pin(Cursor::new(body.as_bytes().to_vec())),
            len: None,
            done: false,
            progress: 0,
        },
        None,
    )
    .await;
    assert_eq!(result, body);
}

#[test(harness)]
async fn static_body_various_buf_sizes() {
    let body = "hello world";
    for size in 3..=body.len() + 4 {
        let result = round_trip_buf(
            BodyType::Static {
                content: body.as_bytes().into(),
                cursor: 0,
            },
            Some(body.len() as u64),
            size,
        )
        .await;
        assert_eq!(result, body, "buf_size={size}");
    }
}

#[test(harness)]
async fn streaming_known_length_various_buf_sizes() {
    let body = "hello streaming world";
    for size in 3..=body.len() + 4 {
        let result = round_trip_buf(
            BodyType::Streaming {
                async_read: Box::pin(Cursor::new(body.as_bytes().to_vec())),
                len: Some(body.len() as u64),
                done: false,
                progress: 0,
            },
            Some(body.len() as u64),
            size,
        )
        .await;
        assert_eq!(result, body, "buf_size={size}");
    }
}

#[test(harness)]
async fn streaming_unknown_length_various_buf_sizes() {
    let body = "hello chunked world";
    for size in 3..=body.len() + 4 {
        let result = round_trip_buf(
            BodyType::Streaming {
                async_read: Box::pin(Cursor::new(body.as_bytes().to_vec())),
                len: None,
                done: false,
                progress: 0,
            },
            None,
            size,
        )
        .await;
        assert_eq!(result, body, "buf_size={size}");
    }
}
