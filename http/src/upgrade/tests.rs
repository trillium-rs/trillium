//! Tests for [`Upgrade`].
//!
//! Focus is wire-format correctness for the h1 chunked and h3 DATA-frame write paths,
//! varying user-side write chunk sizes and transport accept-per-poll caps to exercise
//! the pending-bytes resumption logic. Each round-trip writes through `Upgrade`'s
//! `AsyncWrite` and decodes the wire bytes back.

use super::*;
use crate::{Buffer, Headers, Method, ReceivedBody, Upgrade, Version};
use encoding_rs::UTF_8;
use futures_lite::{AsyncRead, AsyncWrite, AsyncWriteExt, io::Cursor};
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use test_harness::test;
use trillium_testing::harness;

/// In-memory test transport. Records all writes into a shared `Vec<u8>` and exposes them
/// via [`Self::wire`]. The `accept_per_poll` cap exercises `Upgrade`'s pending-
/// bytes resumption: when `Some(n)`, `poll_write` accepts at most `n` bytes per call,
/// forcing the chunked writer's `pending` queue to retain a remainder and drain it on
/// the next poll.
///
/// Always-ready: never returns `Pending`. Tests that need Pending semantics would need
/// a richer fake.
#[derive(Clone, Debug)]
struct RecordingTransport {
    wire: Arc<Mutex<Vec<u8>>>,
    accept_per_poll: Option<usize>,
}

impl RecordingTransport {
    fn new() -> Self {
        Self {
            wire: Arc::new(Mutex::new(Vec::new())),
            accept_per_poll: None,
        }
    }

    fn with_accept_cap(cap: usize) -> Self {
        Self {
            wire: Arc::new(Mutex::new(Vec::new())),
            accept_per_poll: Some(cap),
        }
    }
}

impl AsyncRead for RecordingTransport {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(0))
    }
}

impl AsyncWrite for RecordingTransport {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let n = self.accept_per_poll.unwrap_or(buf.len()).min(buf.len());
        self.wire.lock().unwrap().extend_from_slice(&buf[..n]);
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn h1_upgrade(transport: RecordingTransport) -> Upgrade<RecordingTransport> {
    let mut upgrade = Upgrade::new(
        Headers::new(),
        "/",
        Method::Post,
        transport,
        Buffer::default(),
        Version::Http1_1,
    );
    // Override the default Raw write_state: these tests target the h1 chunked output
    // path. `Upgrade::new` doesn't run compute_write_state, so the upgrade-transition
    // path's H1Chunked selection from `TE: chunked` doesn't fire here.
    upgrade.write_state = WriteState::H1Chunked(H1ChunkedState::default());
    upgrade
}

/// Write `payload` through `framed` in fixed-size slices of `chunk_size` bytes each.
async fn write_with_chunks_of_size(
    upgrade: &mut Upgrade<RecordingTransport>,
    payload: &[u8],
    chunk_size: usize,
) -> io::Result<()> {
    for slice in payload.chunks(chunk_size) {
        upgrade.write_all(slice).await?;
    }
    Ok(())
}

/// Decode chunked wire bytes back into payload + trailers via [`ReceivedBody`].
async fn decode_chunked(wire: Vec<u8>) -> crate::Result<(Vec<u8>, Option<Headers>)> {
    let mut trailers: Option<Headers> = None;
    let body = ReceivedBody::new(
        None,
        Buffer::default(),
        Cursor::new(wire),
        ReceivedBodyState::default(),
        None,
        UTF_8,
    )
    .with_trailers(&mut trailers);
    let bytes = body.read_bytes().await?;
    Ok((bytes, trailers))
}

#[test(harness)]
async fn h1_round_trip_no_trailers_simple() {
    let payload = b"hello world".to_vec();
    let transport = RecordingTransport::new();
    let wire_ref = transport.wire.clone();
    let mut upgrade =h1_upgrade(transport);

    upgrade.write_all(&payload).await.unwrap();
    upgrade.close().await.unwrap();

    let (decoded, trailers) = decode_chunked(wire_ref.lock().unwrap().clone())
        .await
        .unwrap();
    assert_eq!(decoded, payload);
    assert!(trailers.is_none());
}

#[test(harness)]
async fn h1_round_trip_varying_write_chunk_sizes() {
    // Payload spans enough lengths that varying write_chunk_size produces meaningfully
    // different chunk-boundary patterns. The repeat gives ~5400 bytes of payload.
    let payload: Vec<u8> = b"trillium framed upgrade round trip test payload "
        .iter()
        .copied()
        .cycle()
        .take(5400)
        .collect();

    for write_chunk_size in [1, 2, 7, 16, 64, 255, 256, 1024, 4096, 8192] {
        let transport = RecordingTransport::new();
        let wire_ref = transport.wire.clone();
        let mut upgrade =h1_upgrade(transport);

        write_with_chunks_of_size(&mut upgrade, &payload, write_chunk_size)
            .await
            .unwrap();
        upgrade.close().await.unwrap();

        let wire = wire_ref.lock().unwrap().clone();
        let wire_preview = String::from_utf8_lossy(&wire[..wire.len().min(60)]).to_string();
        let (decoded, trailers) = decode_chunked(wire).await.unwrap_or_else(|e| {
            panic!(
                "decode failed for write_chunk_size={write_chunk_size}: {e:?}\nwire preview: \
                 {wire_preview:?}"
            )
        });
        assert_eq!(decoded, payload, "write_chunk_size={write_chunk_size}");
        assert!(trailers.is_none(), "write_chunk_size={write_chunk_size}");
    }
}

#[test(harness)]
async fn h1_round_trip_varying_transport_accept_cap() {
    // accept_per_poll values chosen to land both inside and across chunk-framing
    // boundaries — 1 forces every byte through its own poll, 3 lands mid-hex-header,
    // larger values exercise progressively more relaxed partial-write paths.
    let payload: Vec<u8> = (0..2048).map(|i| (i % 256) as u8).collect();

    for accept_per_poll in [1usize, 2, 3, 5, 17, 64, 4096] {
        for write_chunk_size in [1usize, 16, 256, 2048] {
            let transport = RecordingTransport::with_accept_cap(accept_per_poll);
            let wire_ref = transport.wire.clone();
            let mut upgrade =h1_upgrade(transport);

            write_with_chunks_of_size(&mut upgrade, &payload, write_chunk_size)
                .await
                .unwrap();
            upgrade.close().await.unwrap();

            let (decoded, trailers) = decode_chunked(wire_ref.lock().unwrap().clone())
                .await
                .unwrap();
            assert_eq!(
                decoded, payload,
                "accept_per_poll={accept_per_poll} write_chunk_size={write_chunk_size}"
            );
            assert!(
                trailers.is_none(),
                "accept_per_poll={accept_per_poll} write_chunk_size={write_chunk_size}"
            );
        }
    }
}

#[test(harness)]
async fn h1_send_trailers_round_trip() {
    let payload = b"body before trailers".to_vec();
    let transport = RecordingTransport::new();
    let wire_ref = transport.wire.clone();
    let mut upgrade =h1_upgrade(transport);

    upgrade.write_all(&payload).await.unwrap();

    let mut trailers_out = Headers::new();
    trailers_out.insert("x-checksum", "abc123");
    trailers_out.insert("x-other", "value with spaces");

    upgrade.send_trailers(trailers_out).await.unwrap();

    let (decoded, received_trailers) = decode_chunked(wire_ref.lock().unwrap().clone())
        .await
        .unwrap();
    assert_eq!(decoded, payload);
    let received_trailers = received_trailers.expect("trailers should round-trip");
    assert_eq!(received_trailers.get_str("x-checksum"), Some("abc123"));
    assert_eq!(
        received_trailers.get_str("x-other"),
        Some("value with spaces")
    );
}

#[test(harness)]
async fn h1_send_trailers_under_partial_accept() {
    // Same as above but the transport accepts only 3 bytes per poll — covers the case
    // where part of the `0\r\n` + trailer-section + final CRLF spans multiple drain
    // iterations inside `send_trailers`.
    let payload = b"x".repeat(200);
    let transport = RecordingTransport::with_accept_cap(3);
    let wire_ref = transport.wire.clone();
    let mut upgrade =h1_upgrade(transport);

    write_with_chunks_of_size(&mut upgrade, &payload, 17)
        .await
        .unwrap();

    let mut trailers_out = Headers::new();
    trailers_out.insert("grpc-status", "0");
    trailers_out.insert("grpc-message", "OK");

    upgrade.send_trailers(trailers_out).await.unwrap();

    let (decoded, received_trailers) = decode_chunked(wire_ref.lock().unwrap().clone())
        .await
        .unwrap();
    assert_eq!(decoded, payload);
    let received_trailers = received_trailers.expect("trailers should round-trip");
    assert_eq!(received_trailers.get_str("grpc-status"), Some("0"));
    assert_eq!(received_trailers.get_str("grpc-message"), Some("OK"));
}

#[test(harness)]
async fn h1_write_after_close_errors() {
    let transport = RecordingTransport::new();
    let mut upgrade =h1_upgrade(transport);

    upgrade.write_all(b"hi").await.unwrap();
    upgrade.close().await.unwrap();

    let err = upgrade.write_all(b"more").await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
}

#[test(harness)]
async fn h1_send_trailers_after_close_errors() {
    let transport = RecordingTransport::new();
    let mut upgrade =h1_upgrade(transport);

    upgrade.write_all(b"hi").await.unwrap();
    upgrade.close().await.unwrap();

    let trailers = Headers::new();
    let err = upgrade.send_trailers(trailers).await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
}

#[test(harness)]
async fn h1_empty_payload_close_emits_terminator_only() {
    // Closing without any prior writes should still produce a valid chunked body
    // (just the terminator `0\r\n\r\n`), which decodes to empty payload + no trailers.
    let transport = RecordingTransport::new();
    let wire_ref = transport.wire.clone();
    let mut upgrade =h1_upgrade(transport);

    upgrade.close().await.unwrap();

    let (decoded, trailers) = decode_chunked(wire_ref.lock().unwrap().clone())
        .await
        .unwrap();
    assert!(decoded.is_empty());
    assert!(trailers.is_none());
}

#[test(harness)]
async fn h1_vectored_write_emits_single_chunk() {
    use futures_lite::io::AsyncWriteExt;
    use std::io::IoSlice;

    // The h1 chunked path's `poll_write_vectored` coalesces all bufs into a single chunk:
    // one chunk-size header + the concatenated payload + one CRLF. Default `AsyncWrite`
    // vectored shim would have written one chunk per buf.
    let transport = RecordingTransport::new();
    let wire_ref = transport.wire.clone();
    let mut upgrade =h1_upgrade(transport);

    let parts: [&[u8]; 3] = [b"alpha-", b"beta-", b"gamma"];
    let slices: Vec<IoSlice<'_>> = parts.iter().map(|p| IoSlice::new(p)).collect();
    let total: usize = parts.iter().map(|p| p.len()).sum();
    let n = upgrade.write_vectored(&slices).await.unwrap();
    assert_eq!(n, total);
    upgrade.close().await.unwrap();

    let wire = wire_ref.lock().unwrap().clone();
    let (decoded, _) = decode_chunked(wire.clone()).await.unwrap();
    assert_eq!(decoded, b"alpha-beta-gamma");
    // Exactly one chunk + the terminator on the wire — verify the chunk-size header is
    // the *combined* length, not one of the part lengths.
    let header_prefix = format!("{total:X}\r\n");
    assert!(
        wire.starts_with(header_prefix.as_bytes()),
        "expected chunk header {header_prefix:?} at start, got wire={:?}",
        String::from_utf8_lossy(&wire[..wire.len().min(40)])
    );
}

/// Decode raw HTTP/3 DATA frame bytes from `wire` into a concatenated payload + a count of
/// DATA frames seen. Errors if any non-DATA frame appears.
fn decode_h3_data_frames(wire: &[u8]) -> (Vec<u8>, usize) {
    use crate::h3::Frame;

    let mut payload = Vec::new();
    let mut frame_count = 0;
    let mut cursor = 0;
    while cursor < wire.len() {
        let (frame, header_len) = Frame::decode(&wire[cursor..]).unwrap_or_else(|e| {
            panic!("Frame::decode failed at offset {cursor}: {e:?}");
        });
        cursor += header_len;
        let Frame::Data(n) = frame else {
            panic!("expected only DATA frames, got {frame:?}");
        };
        let n = n as usize;
        assert!(
            cursor + n <= wire.len(),
            "DATA frame payload ({n}) extends past end of wire ({left} remaining)",
            left = wire.len() - cursor,
        );
        payload.extend_from_slice(&wire[cursor..cursor + n]);
        cursor += n;
        frame_count += 1;
    }
    (payload, frame_count)
}

fn h3_upgrade(transport: RecordingTransport) -> Upgrade<RecordingTransport> {
    let mut upgrade = Upgrade::new(
        Headers::new(),
        "/",
        Method::Post,
        transport,
        Buffer::default(),
        Version::Http3,
    );
    // Override the default Raw write_state: h3 should always be framed, but
    // `Upgrade::new` defaults to Raw since it doesn't run compute_write_state.
    upgrade.write_state = WriteState::H3Framed(H3FramedState::default());
    upgrade
}

#[test(harness)]
async fn h3_round_trip_data_frames_simple() {
    let payload = b"hello h3 framed upgrade".to_vec();
    let transport = RecordingTransport::new();
    let wire_ref = transport.wire.clone();
    let mut upgrade =h3_upgrade(transport);

    upgrade.write_all(&payload).await.unwrap();
    upgrade.close().await.unwrap();

    let wire = wire_ref.lock().unwrap().clone();
    let (decoded, count) = decode_h3_data_frames(&wire);
    assert_eq!(decoded, payload);
    assert_eq!(
        count, 1,
        "single write_all should produce exactly one DATA frame"
    );
}

#[test(harness)]
async fn h3_data_frame_per_poll_write() {
    // Each non-vectored `poll_write` emits its own DATA frame. Five chunked writes →
    // five DATA frames.
    let payload: Vec<u8> = (0..200).map(|i| (i % 256) as u8).collect();
    let transport = RecordingTransport::new();
    let wire_ref = transport.wire.clone();
    let mut upgrade =h3_upgrade(transport);

    for slice in payload.chunks(40) {
        upgrade.write_all(slice).await.unwrap();
    }
    upgrade.close().await.unwrap();

    let wire = wire_ref.lock().unwrap().clone();
    let (decoded, count) = decode_h3_data_frames(&wire);
    assert_eq!(decoded, payload);
    assert_eq!(count, 5, "expected one DATA frame per write_all call");
}

#[test(harness)]
async fn h3_data_frame_under_partial_transport_accept() {
    // accept_per_poll=3 forces partial-write resumption mid-DATA-frame-header AND mid-payload.
    let payload: Vec<u8> = (0..512).map(|i| (i % 256) as u8).collect();
    let transport = RecordingTransport::with_accept_cap(3);
    let wire_ref = transport.wire.clone();
    let mut upgrade =h3_upgrade(transport);

    for slice in payload.chunks(17) {
        upgrade.write_all(slice).await.unwrap();
    }
    upgrade.close().await.unwrap();

    let wire = wire_ref.lock().unwrap().clone();
    let (decoded, _count) = decode_h3_data_frames(&wire);
    assert_eq!(decoded, payload);
}

#[test(harness)]
async fn h3_vectored_writes_single_frame() {
    use futures_lite::io::AsyncWriteExt;
    use std::io::IoSlice;

    let parts: [&[u8]; 4] = [b"len:", b"0005:", b"hello", b"!"];
    let total: usize = parts.iter().map(|p| p.len()).sum();
    let transport = RecordingTransport::new();
    let wire_ref = transport.wire.clone();
    let mut upgrade =h3_upgrade(transport);

    let slices: Vec<IoSlice<'_>> = parts.iter().map(|p| IoSlice::new(p)).collect();
    let n = upgrade.write_vectored(&slices).await.unwrap();
    assert_eq!(n, total);
    upgrade.close().await.unwrap();

    let wire = wire_ref.lock().unwrap().clone();
    let (decoded, count) = decode_h3_data_frames(&wire);
    assert_eq!(decoded, b"len:0005:hello!");
    assert_eq!(
        count, 1,
        "vectored write must coalesce all iobufs into one DATA frame"
    );
}

#[test(harness)]
async fn h3_empty_payload_close_writes_nothing() {
    // h3 close maps to QUIC FIN via `transport.poll_close` — no terminator frame written.
    // Our RecordingTransport ignores close; an empty-payload close should leave the wire
    // empty.
    let transport = RecordingTransport::new();
    let wire_ref = transport.wire.clone();
    let mut upgrade =h3_upgrade(transport);

    upgrade.close().await.unwrap();

    let wire = wire_ref.lock().unwrap().clone();
    assert!(
        wire.is_empty(),
        "h3 close on empty stream should not write any framing bytes, got {} bytes",
        wire.len()
    );
}

#[test(harness)]
async fn h3_write_after_close_errors() {
    let transport = RecordingTransport::new();
    let mut upgrade =h3_upgrade(transport);

    upgrade.write_all(b"first").await.unwrap();
    upgrade.close().await.unwrap();

    let err = upgrade.write_all(b"more").await.unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
}

#[test(harness)]
async fn h3_send_trailers_after_close_errors() {
    let transport = RecordingTransport::new();
    let mut upgrade =h3_upgrade(transport);

    upgrade.close().await.unwrap();

    let err = upgrade
        .send_trailers(Headers::new())
        .await
        .expect_err("send_trailers after close should error");
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
}

#[test(harness)]
async fn h3_send_trailers_without_h3_protocol_session_errors() {
    // `Upgrade::new` always sets `protocol_session: ProtocolSession::Http1`, so the h3
    // arm of `send_trailers` finds no live connection via `as_h3()` and surfaces
    // `NotConnected`. The DATA-frame tests above don't exercise this path.
    let transport = RecordingTransport::new();
    let upgrade =h3_upgrade(transport);

    let err = upgrade
        .send_trailers(Headers::new())
        .await
        .expect_err("send_trailers with no h3 session should error");
    assert_eq!(err.kind(), io::ErrorKind::NotConnected);
}
