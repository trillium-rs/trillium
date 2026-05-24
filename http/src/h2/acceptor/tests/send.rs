//! Send-pump framing tests: the fiddly byte-level work in [`send`][super::super::send] that
//! turns a staged response into HEADERS / DATA / trailing-HEADERS frames. These exercise
//! cells the lifecycle / flow-control / shutdown suites don't reach — owned-body trailers
//! (the gRPC-unary path, distinct from the upgrade `submit_trailers` ring), HEADERS that
//! must fragment into CONTINUATION, the connection-level send window as the binding
//! constraint, a zero-length declared body, and a body source that parks before yielding.

use super::fixture::*;
use crate::{
    Body, BodySource, Headers, Method, Status,
    h2::{frame::Frame, settings::H2Settings},
    headers::hpack::PseudoHeaders,
};
use futures_lite::io::AsyncRead;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

/// Sum the DATA payload bytes across a decoded frame batch for `stream_id`.
fn data_bytes(frames: &[Frame], stream_id: u32) -> u32 {
    frames
        .iter()
        .filter_map(|f| match f {
            Frame::Data {
                stream_id: id,
                data_length,
                ..
            } if *id == stream_id => Some(*data_length),
            _ => None,
        })
        .sum()
}

/// A streaming [`BodySource`] that yields `data` in full, then surfaces `trailers` once the
/// reader hits EOF — modeling a response body that computes trailers from its own bytes (the
/// gRPC-unary `grpc-status` pattern, or a streamed-content hash).
struct BodyWithTrailers {
    data: &'static [u8],
    pos: usize,
    trailers: Option<Headers>,
}

impl AsyncRead for BodyWithTrailers {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let n = (this.data.len() - this.pos).min(buf.len());
        buf[..n].copy_from_slice(&this.data[this.pos..this.pos + n]);
        this.pos += n;
        Poll::Ready(Ok(n))
    }
}

impl BodySource for BodyWithTrailers {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        self.get_mut().trailers.take()
    }
}

/// A streaming [`BodySource`] that returns `Pending` on its first `poll_read` (without
/// registering a waker — the fixture re-polls each `tick`), then yields `data`, then EOF.
/// Models a response body that isn't ready the instant the headers are framed (a proxied
/// upstream, a slow generator).
struct PendingThenBody {
    data: &'static [u8],
    pos: usize,
    pended: bool,
}

impl AsyncRead for PendingThenBody {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if !this.pended {
            this.pended = true;
            return Poll::Pending;
        }
        let n = (this.data.len() - this.pos).min(buf.len());
        buf[..n].copy_from_slice(&this.data[this.pos..this.pos + n]);
        this.pos += n;
        Poll::Ready(Ok(n))
    }
}

impl BodySource for PendingThenBody {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        None
    }
}

/// A response whose owned `Body` carries trailers must frame the trailers as a trailing
/// HEADERS(END_STREAM) block *in place of* the empty `DATA(END_STREAM)` terminator — never
/// both. This is the gRPC-unary response path: `submit_send` stages `[Headers, Body, Close]`,
/// and when the body drains with trailers attached the pump splices `Trailers` in, dropping
/// the bare `Close` ([`drain_body_into_trailers`][super::super::send]). Distinct from the
/// upgrade path (`submit_trailers` → streaming ring), which the lifecycle suite covers; here
/// the trailers ride the owned body the framework hands to the driver.
#[test]
fn owned_body_trailers_replace_empty_end_stream_terminator() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    let mut trailers = Headers::new();
    trailers.insert("grpc-status", "0");
    let body = Body::new_with_trailers(
        BodyWithTrailers {
            data: b"hello",
            pos: 0,
            trailers: Some(trailers),
        },
        None,
    );
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx
        .connection
        .submit_send(1, pseudos, Headers::new(), Some(body));

    // Collect the whole closing sequence (HEADERS, body DATA, trailing HEADERS) — it fits in
    // one tick's round budget, but draining a few keeps the test robust to scheduling.
    let mut frames = Vec::new();
    for _ in 0..4 {
        let _ = fx.tick();
        frames.extend(fx.next_outbound_frames());
    }

    let header_frames: Vec<bool> = frames
        .iter()
        .filter_map(|f| match f {
            Frame::Headers {
                stream_id: 1,
                end_stream,
                ..
            } => Some(*end_stream),
            _ => None,
        })
        .collect();
    assert_eq!(
        header_frames,
        vec![false, true],
        "expected response HEADERS (no END_STREAM) followed by trailing HEADERS(END_STREAM); got \
         header frames {header_frames:?} in {frames:?}",
    );
    assert_eq!(
        data_bytes(&frames, 1),
        5,
        "the body's 5 bytes should frame as DATA; got {frames:?}",
    );
    assert!(
        !frames.iter().any(|f| matches!(
            f,
            Frame::Data {
                stream_id: 1,
                end_stream: true,
                ..
            }
        )),
        "trailers must replace the empty DATA(END_STREAM) terminator — no END_STREAM DATA frame \
         should appear; got {frames:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "stream should be removed once the trailing HEADERS terminator is framed",
    );
}

/// A response header block larger than the peer's `MAX_FRAME_SIZE` must fragment into a
/// HEADERS frame followed by one or more CONTINUATION frames, with `END_STREAM` carried on the
/// *first* fragment only and `END_HEADERS` on the *last* ([`emit_headers_block`]'s
/// fragmentation loop). Bodyless response so the stream terminates on the HEADERS block itself
/// — pinning that the folded END_STREAM rides the leading HEADERS even when the block spans
/// CONTINUATION frames.
///
/// [`emit_headers_block`]: super::super::send
#[test]
fn large_header_block_fragments_into_continuation() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // A single ~40 KiB header value. The default effective MAX_FRAME_SIZE is 16384, so the
    // encoded block can't fit one frame regardless of HPACK literal/Huffman choices.
    let mut headers = Headers::new();
    headers.insert("x-large", "abcdefghij".repeat(4000));
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_send(1, pseudos, headers, None);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    let leading = frames.iter().find_map(|f| match f {
        Frame::Headers {
            stream_id: 1,
            end_stream,
            end_headers,
            ..
        } => Some((*end_stream, *end_headers)),
        _ => None,
    });
    assert_eq!(
        leading,
        Some((true, false)),
        "leading HEADERS should carry END_STREAM (bodyless response) but NOT END_HEADERS (block \
         continues in CONTINUATION); got {frames:?}",
    );

    let continuations: Vec<bool> = frames
        .iter()
        .filter_map(|f| match f {
            Frame::Continuation {
                stream_id: 1,
                end_headers,
                ..
            } => Some(*end_headers),
            _ => None,
        })
        .collect();
    assert!(
        !continuations.is_empty(),
        "an over-large header block must emit at least one CONTINUATION; got {frames:?}",
    );
    assert_eq!(
        continuations.last(),
        Some(&true),
        "the final CONTINUATION must set END_HEADERS; got continuations {continuations:?}",
    );
    assert!(
        continuations[..continuations.len() - 1].iter().all(|e| !e),
        "only the last fragment may set END_HEADERS; got {continuations:?}",
    );
}

/// The *connection*-level send window — fixed at 65535 until the peer credits it, independent
/// of the per-stream window — is enforced as the binding constraint. With a generous per-stream
/// window but a body exceeding 65535, the pump frames exactly the connection window's worth,
/// then parks (`has_pending_outbound_progress` returns false on a non-positive connection
/// window) without sending END_STREAM; a connection-level `WINDOW_UPDATE` (stream 0) resumes
/// it. Distinct from the per-stream exhaustion the flow-control suite covers.
#[test]
fn connection_send_window_exhaustion_parks_then_resumes() {
    let mut fx = DriverFixture::new_server();
    // Peer grants a large per-stream window, so only the 65535 connection window can bind.
    fx.complete_handshake_with_peer_settings(
        H2Settings::default().with_initial_window_size(200_000),
    );

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // 70000-byte body against a 65535 connection window.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_send(
        1,
        pseudos,
        Headers::new(),
        Some(Body::new_static(vec![b'x'; 70_000])),
    );
    let _ = fx.tick();

    let first = fx.next_outbound_frames();
    assert_eq!(
        data_bytes(&first, 1),
        65_535,
        "pump should frame exactly the connection window (65535), then park; got {first:?}",
    );
    assert!(
        !first.iter().any(|f| matches!(
            f,
            Frame::Data {
                stream_id: 1,
                end_stream: true,
                ..
            }
        )),
        "no END_STREAM while the connection window is exhausted mid-body; got {first:?}",
    );
    assert!(
        fx.connection.streams_lock().contains_key(&1),
        "stream must stay live while parked on a zero connection window",
    );

    // Credit the connection window (stream 0). The pump resumes and frames the remaining 4465
    // bytes + END_STREAM.
    fx.peer_window_update(0, 100_000);
    let _ = fx.tick();
    let after = fx.next_outbound_frames();
    assert_eq!(
        data_bytes(&after, 1),
        70_000 - 65_535,
        "after the connection WINDOW_UPDATE the pump should frame the body's remainder; got \
         {after:?}",
    );
    assert!(
        after.iter().any(|f| matches!(
            f,
            Frame::Data {
                stream_id: 1,
                end_stream: true,
                ..
            }
        )),
        "resumed send should terminate with END_STREAM; got {after:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "with the body fully sent and recv already closed, the stream should be removed",
    );
}

/// A present-but-empty body (`Some(Body::new_static(b""))`, declared length 0) takes
/// `poll_emit_body`'s fast path: declared length already satisfied at entry, so the cursor
/// transitions out of the body without a `poll_read` and without parking for a window that
/// would never be needed. The empty body frames no DATA of its own; the stream terminates with
/// the staged `Close` as an empty `DATA(END_STREAM)`.
#[test]
fn empty_declared_body_frames_no_data_then_terminates() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_send(
        1,
        pseudos,
        Headers::new(),
        Some(Body::new_static(b"" as &[u8])),
    );
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        frames
            .iter()
            .any(|f| matches!(f, Frame::Headers { stream_id: 1, .. })),
        "response HEADERS should be framed; got {frames:?}",
    );
    assert_eq!(
        data_bytes(&frames, 1),
        0,
        "an empty body must frame no DATA payload; got {frames:?}",
    );
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::Data {
                stream_id: 1,
                end_stream: true,
                data_length: 0,
                ..
            }
        )),
        "the stream should terminate with an empty DATA(END_STREAM); got {frames:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "the completed stream should be removed",
    );
}

/// A response body whose source returns `Pending` before it has bytes ready must not stall the
/// stream or emit a premature terminator: the pump frames the response HEADERS, parks on the
/// body (`poll_emit_body` → `Pending`), and resumes framing DATA once the source yields. Pins
/// that an owned streaming body that isn't immediately ready behaves like the flow-control
/// park — HEADERS now, DATA later — rather than racing END_STREAM out early.
#[test]
fn pending_body_source_parks_then_resumes() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    let body = Body::new_streaming(
        PendingThenBody {
            data: b"stream-body",
            pos: 0,
            pended: false,
        },
        None,
    );
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx
        .connection
        .submit_send(1, pseudos, Headers::new(), Some(body));

    // First tick: HEADERS go out, the body's first poll_read returns Pending → cursor parks.
    let _ = fx.tick();
    let first = fx.next_outbound_frames();
    assert!(
        first
            .iter()
            .any(|f| matches!(f, Frame::Headers { stream_id: 1, .. })),
        "response HEADERS should be framed on the first tick; got {first:?}",
    );
    assert_eq!(
        data_bytes(&first, 1),
        0,
        "no DATA should be framed while the body source is still Pending; got {first:?}",
    );
    assert!(
        fx.connection.streams_lock().contains_key(&1),
        "stream must stay live while parked on a not-yet-ready body",
    );

    // Second tick: the source now yields, so DATA + END_STREAM frame and the stream completes.
    let _ = fx.tick();
    let after = fx.next_outbound_frames();
    assert_eq!(
        data_bytes(&after, 1),
        b"stream-body".len() as u32,
        "the resumed body should frame its bytes as DATA; got {after:?}",
    );
    assert!(
        after.iter().any(|f| matches!(
            f,
            Frame::Data {
                stream_id: 1,
                end_stream: true,
                ..
            }
        )),
        "the resumed send should terminate with END_STREAM; got {after:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "the completed stream should be removed",
    );
}
