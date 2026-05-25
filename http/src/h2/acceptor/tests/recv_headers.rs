//! Inbound HEADERS handling: CONTINUATION accumulation (the receive-side dual of the
//! [`send`][super::super::send] fragmentation path, including the CONTINUATION-flood DoS
//! guard), stream admission (concurrency limit), and §8.1.2 request validation. The peer
//! fixture's valid-shape helpers always set `END_HEADERS`, so the accumulation tests use
//! [`DriverFixture::peer_open_stream_no_end_headers`] / [`DriverFixture::peer_continuation`] /
//! [`DriverFixture::peer_open_stream_split`] to drive a multi-frame header block.

use super::fixture::*;
use crate::{
    Headers, HttpConfig, KnownHeaderName, Method, Status,
    h2::{H2ErrorCode, frame::Frame},
    headers::hpack::PseudoHeaders,
};
use std::task::Poll;

/// True if any frame in the batch is a `GOAWAY` carrying `code`.
fn goaway_with(frames: &[Frame], code: H2ErrorCode) -> bool {
    frames
        .iter()
        .any(|f| matches!(f, Frame::Goaway { error_code, .. } if *error_code == code))
}

/// True if any frame in the batch is a `RST_STREAM` on `stream_id` carrying `code`.
fn rst_with(frames: &[Frame], stream_id: u32, code: H2ErrorCode) -> bool {
    frames.iter().any(|f| {
        matches!(f, Frame::RstStream { stream_id: s, error_code }
            if *s == stream_id && *error_code == code)
    })
}

/// Pseudo-headers + fields for a POST carrying a `content-length` declaration, for the
/// §8.1.2.6 cross-check tests below.
fn post_with_content_length(content_length: u64) -> (PseudoHeaders<'static>, Headers) {
    let pseudos = PseudoHeaders::default()
        .with_method(Method::Post)
        .with_path("/")
        .with_scheme("http")
        .with_authority("test");
    let mut fields = Headers::new();
    fields.insert(KnownHeaderName::ContentLength, content_length.to_string());
    (pseudos, fields)
}

/// A peer that opens a header block (HEADERS without `END_HEADERS`) and then floods
/// CONTINUATION frames without ever ending it would grow the driver's accumulation buffer
/// without bound — the HTTP/2 CONTINUATION-flood DoS (CVE-2024-27316 class). The driver caps
/// the cumulative *compressed* block at the advertised `MAX_HEADER_LIST_SIZE` and closes the
/// connection with `GOAWAY(ENHANCE_YOUR_CALM)` once a fragment would push it past the cap —
/// before decoding, so a flood of junk bytes can't OOM us. Uses a small configured cap so the
/// test sends a few hundred bytes rather than tens of KiB.
#[test]
fn continuation_flood_past_max_header_list_size_enhances_calm() {
    let mut fx =
        DriverFixture::new_server_with_config(HttpConfig::default().with_max_header_list_size(100));
    fx.complete_handshake();

    // Open a new request stream but leave the header block open (no END_HEADERS).
    fx.peer_open_stream_no_end_headers(1, Method::Post, "/", false);
    // A CONTINUATION carrying more bytes than the 100-byte accumulation cap allows.
    fx.peer_continuation(1, &[0u8; 200], false);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        goaway_with(&frames, H2ErrorCode::EnhanceYourCalm),
        "a header block exceeding MAX_HEADER_LIST_SIZE across CONTINUATION frames must \
         GOAWAY(ENHANCE_YOUR_CALM); got {frames:?}",
    );
}

/// The happy path: a request whose header block is split across a HEADERS frame (no
/// `END_HEADERS`) and a CONTINUATION frame (`END_HEADERS`) must reassemble, HPACK-decode, and
/// yield the request `Conn` — identical to a single-frame request. Exercises the inbound
/// accumulation + finalize-from-CONTINUATION path that the peer fixture's `END_HEADERS`-baking
/// helpers otherwise never reach.
#[test]
fn request_header_block_split_across_continuation_yields_conn() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Split early (3 bytes in HEADERS, the rest in CONTINUATION) to force real reassembly.
    fx.peer_open_stream_split(1, Method::Get, "/split", true, 3);
    match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => {
            assert_eq!(conn.method(), Method::Get);
            assert_eq!(conn.path(), "/split");
        }
        other => panic!(
            "a header block reassembled from HEADERS + CONTINUATION should yield the request \
             Conn; got {other:?}",
        ),
    }
}

/// A CONTINUATION frame with no header block in progress is a connection-level
/// `PROTOCOL_ERROR` (RFC 9113 §6.10 — CONTINUATION may only follow a HEADERS/PUSH_PROMISE
/// without `END_HEADERS`).
#[test]
fn continuation_without_open_header_block_is_protocol_error() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_continuation(1, &[0u8; 4], true);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        goaway_with(&frames, H2ErrorCode::ProtocolError),
        "a CONTINUATION with no in-progress header block must be a connection PROTOCOL_ERROR; got \
         {frames:?}",
    );
}

/// A CONTINUATION frame whose stream id doesn't match the in-progress header block is a
/// connection-level `PROTOCOL_ERROR` (§6.10 — the continuation must be for the same stream).
#[test]
fn continuation_on_mismatched_stream_is_protocol_error() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Open a block on stream 1, then send a CONTINUATION naming stream 3.
    fx.peer_open_stream_no_end_headers(1, Method::Post, "/", false);
    fx.peer_continuation(3, &[0u8; 4], true);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        goaway_with(&frames, H2ErrorCode::ProtocolError),
        "a CONTINUATION on a different stream than the in-progress block must be a connection \
         PROTOCOL_ERROR; got {frames:?}",
    );
}

/// Any non-CONTINUATION frame arriving while a header block is in progress is a
/// connection-level `PROTOCOL_ERROR` (§6.10 — the block must complete before any other frame,
/// on any stream). Here a DATA frame interleaves an open block.
#[test]
fn non_continuation_frame_during_header_block_is_protocol_error() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream_no_end_headers(1, Method::Post, "/", false);
    fx.peer_data(1, b"x", false);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        goaway_with(&frames, H2ErrorCode::ProtocolError),
        "a non-CONTINUATION frame interleaving an in-progress header block must be a connection \
         PROTOCOL_ERROR; got {frames:?}",
    );
}

/// A peer-initiated stream past our advertised `SETTINGS_MAX_CONCURRENT_STREAMS` is refused
/// with `RST_STREAM(REFUSED_STREAM)` — the server's admission backpressure — without yielding
/// a `Conn` or registering the stream. The first stream is held open (no response) to occupy
/// the single permitted slot.
#[test]
fn exceeding_max_concurrent_streams_refuses_with_rst() {
    let mut fx = DriverFixture::new_server_with_config(
        HttpConfig::default().with_h2_max_concurrent_streams(1),
    );
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn for stream 1, got {other:?}"),
    };

    // Second concurrent stream exceeds the limit of 1.
    fx.peer_open_stream(3, Method::Get, "/", true);
    let polled = fx.tick();
    assert!(
        !matches!(polled, Poll::Ready(Some(Ok(_)))),
        "a stream past MAX_CONCURRENT_STREAMS must not yield a Conn; got {polled:?}",
    );

    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::RstStream {
                stream_id: 3,
                error_code: H2ErrorCode::RefusedStream,
            }
        )),
        "the excess stream must be refused with RST_STREAM(REFUSED_STREAM); got {frames:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&3),
        "the refused stream must not be registered",
    );
}

/// A malformed request (§8.1.2 — here an HTTP/1-only connection-specific header) is rejected
/// during `Conn` construction with `RST_STREAM(PROTOCOL_ERROR)` *before* any handler task sees
/// it, leaving the connection alive. Pins that validation happens at the driver, not in the
/// handler.
#[test]
fn malformed_request_rejected_before_handler_with_rst() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Complete pseudos, but a forbidden connection-specific header (RFC 9113 §8.2.2).
    let pseudos = PseudoHeaders::default()
        .with_method(Method::Get)
        .with_path("/")
        .with_scheme("http")
        .with_authority("test");
    let mut fields = Headers::new();
    fields.insert("connection", "close");
    fx.peer_headers(1, pseudos, &fields, true);

    let polled = fx.tick();
    assert!(
        !matches!(polled, Poll::Ready(Some(Ok(_)))),
        "a malformed request must not yield a Conn to a handler; got {polled:?}",
    );

    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::RstStream {
                stream_id: 1,
                error_code: H2ErrorCode::ProtocolError,
            }
        )),
        "a malformed request must be rejected with RST_STREAM(PROTOCOL_ERROR); got {frames:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "the rejected stream must not be registered",
    );
}

/// A second HEADERS frame on a stream whose recv half is already closed (the peer sent
/// `END_STREAM` on its opening HEADERS) is a stream-level `STREAM_CLOSED` (RFC 9113 §5.1) —
/// the stream is reset and removed. This is the HEADERS-after-recv-close dual of
/// `peer_data_after_its_own_end_stream_is_reset` (which tests *DATA* after END_STREAM); the
/// HEADERS path reaches the recv-closed guard before the trailer-validation path.
#[test]
fn headers_on_half_closed_remote_stream_is_stream_closed() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Body-less request: recv half closes at the opening HEADERS → HalfClosedRemote.
    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn for stream 1, got {other:?}"),
    };
    let _ = fx.next_outbound_frames();

    // A further (trailer-shaped) HEADERS on the recv-closed stream is illegal.
    let mut trailers = Headers::new();
    trailers.insert("grpc-status", "0");
    fx.peer_trailers(1, &trailers);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::RstStream {
                stream_id: 1,
                error_code: H2ErrorCode::StreamClosed,
            }
        )),
        "HEADERS on a half-closed-remote stream must earn RST_STREAM(STREAM_CLOSED); got \
         {frames:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "the stream should be removed after the illegal HEADERS",
    );
}

/// A peer-initiated stream must use an odd id (RFC 9113 §5.1.1 — even ids are server-initiated
/// pushes, which trillium never opens). An even id is a connection-level `PROTOCOL_ERROR`.
#[test]
fn even_peer_stream_id_is_protocol_error() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(2, Method::Get, "/", true);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        goaway_with(&frames, H2ErrorCode::ProtocolError),
        "an even peer stream id must be a connection PROTOCOL_ERROR; got {frames:?}",
    );
}

/// HEADERS on a stream we previously reset (recorded in the closed-streams ledger as `Reset`)
/// is a *stream-level* `STREAM_CLOSED` — the peer sent on a stream we tore down. The connection
/// stays alive (the peer may simply not have seen our RST yet).
#[test]
fn headers_on_reset_stream_is_stream_level_stream_closed() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn for stream 1, got {other:?}"),
    };

    // We learn the peer reset it; the ledger records stream 1 as Reset and removes it.
    fx.peer_rst_stream(1, H2ErrorCode::Cancel);
    let _ = fx.tick();
    assert!(!fx.connection.streams_lock().contains_key(&1));
    let _ = fx.next_outbound_frames();

    // The peer (not yet aware) sends fresh HEADERS on that id.
    fx.peer_open_stream(1, Method::Post, "/", true);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::RstStream {
                stream_id: 1,
                error_code: H2ErrorCode::StreamClosed,
            }
        )),
        "HEADERS on a reset stream must earn a stream-level RST_STREAM(STREAM_CLOSED); got \
         {frames:?}",
    );
    assert!(
        !frames.iter().any(|f| matches!(f, Frame::Goaway { .. })),
        "a stream we reset must not escalate to a connection error; got {frames:?}",
    );
}

/// HEADERS on a stream that closed *cleanly* (both halves END_STREAM, ledger `EndStream`) is a
/// connection-level `STREAM_CLOSED` → GOAWAY (RFC 9113 §5.1 — a clean-closed stream is fully
/// done; reopening it is a connection-fatal protocol violation, distinct from the lenient
/// reset case).
#[test]
fn headers_on_cleanly_closed_stream_goaways_stream_closed() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Body-less request + body-less response → both halves END_STREAM → removed, ledger EndStream.
    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn for stream 1, got {other:?}"),
    };
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_send(1, pseudos, Headers::new(), None);
    let _ = fx.tick();
    assert!(!fx.connection.streams_lock().contains_key(&1));
    let _ = fx.next_outbound_frames();

    // The peer sends HEADERS on the cleanly-closed id.
    fx.peer_open_stream(1, Method::Get, "/", true);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        goaway_with(&frames, H2ErrorCode::StreamClosed),
        "HEADERS on a cleanly-closed stream must be a connection STREAM_CLOSED; got {frames:?}",
    );
}

/// HEADERS on a never-opened id *below* `last_peer_stream_id` (implicitly closed by a
/// higher-id stream, and absent from the ledger) is a connection-level `PROTOCOL_ERROR` (RFC
/// 9113 §5.1.1 — stream ids must be monotonically increasing).
#[test]
fn headers_on_never_opened_lower_id_is_protocol_error() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Open stream 3 first, advancing last_peer_stream_id to 3.
    fx.peer_open_stream(3, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn for stream 3, got {other:?}"),
    };
    let _ = fx.next_outbound_frames();

    // Stream 1 was never opened and is below last_peer_stream_id — implicitly closed.
    fx.peer_open_stream(1, Method::Get, "/", true);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        goaway_with(&frames, H2ErrorCode::ProtocolError),
        "HEADERS on a never-opened lower id must be a connection PROTOCOL_ERROR; got {frames:?}",
    );
}

/// RFC 9113 §8.1.2.6: a single DATA frame longer than the declared `content-length` is a
/// stream-level `PROTOCOL_ERROR`, caught the moment the running tally passes the declared
/// length — before END_STREAM. Mirrors h2spec http2/8.1.2.6 case 1.
#[test]
fn data_exceeding_content_length_is_protocol_error() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    let (pseudos, fields) = post_with_content_length(1);
    fx.peer_headers(1, pseudos, &fields, false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn for stream 1, got {other:?}"),
    };
    let _ = fx.next_outbound_frames();

    // Declared 1, send 4 with END_STREAM.
    fx.peer_data(1, b"test", true);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        rst_with(&frames, 1, H2ErrorCode::ProtocolError),
        "DATA longer than content-length must earn RST_STREAM(PROTOCOL_ERROR); got {frames:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "the stream should be removed after the content-length violation",
    );
}

/// RFC 9113 §8.1.2.6: a body *shorter* than the declared `content-length` is also a
/// stream-level `PROTOCOL_ERROR` — detected at END_STREAM, where the final tally still
/// disagrees with the declared length.
#[test]
fn data_short_of_content_length_at_end_stream_is_protocol_error() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    let (pseudos, fields) = post_with_content_length(10);
    fx.peer_headers(1, pseudos, &fields, false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn for stream 1, got {other:?}"),
    };
    let _ = fx.next_outbound_frames();

    // Declared 10, send 4 with END_STREAM — the body ends short.
    fx.peer_data(1, b"test", true);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        rst_with(&frames, 1, H2ErrorCode::ProtocolError),
        "a body short of content-length must earn RST_STREAM(PROTOCOL_ERROR) at END_STREAM; got \
         {frames:?}",
    );
    assert!(!fx.connection.streams_lock().contains_key(&1));
}

/// RFC 9113 §8.1.2.6: the declared length is checked against the *sum* of DATA frames. A
/// first frame within budget followed by a second that overshoots is a stream-level
/// `PROTOCOL_ERROR`. Mirrors h2spec http2/8.1.2.6 case 2.
#[test]
fn multiple_data_frames_summing_past_content_length_is_protocol_error() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    let (pseudos, fields) = post_with_content_length(5);
    fx.peer_headers(1, pseudos, &fields, false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn for stream 1, got {other:?}"),
    };
    let _ = fx.next_outbound_frames();

    // 4 (within the declared 5), then another 4 → sum 8 > 5.
    fx.peer_data(1, b"test", false);
    let _ = fx.tick();
    fx.peer_data(1, b"test", true);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        rst_with(&frames, 1, H2ErrorCode::ProtocolError),
        "DATA frames summing past content-length must earn RST_STREAM(PROTOCOL_ERROR); got \
         {frames:?}",
    );
    assert!(!fx.connection.streams_lock().contains_key(&1));
}

/// Control case for the §8.1.2.6 checks: a body whose length exactly matches the declared
/// `content-length` is well-formed — no RST, and the stream survives (recv half closes to
/// half-closed-remote, awaiting the handler's response).
#[test]
fn data_matching_content_length_is_accepted() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    let (pseudos, fields) = post_with_content_length(4);
    fx.peer_headers(1, pseudos, &fields, false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn for stream 1, got {other:?}"),
    };
    let _ = fx.next_outbound_frames();

    fx.peer_data(1, b"test", true);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        !rst_with(&frames, 1, H2ErrorCode::ProtocolError),
        "a body matching content-length must not be reset; got {frames:?}",
    );
    assert!(
        fx.connection.streams_lock().contains_key(&1),
        "a well-formed request stream should remain open for its response",
    );
}
