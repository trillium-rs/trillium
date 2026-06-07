//! Flow-control wire tests: per-stream send-window exhaustion + resume, `WINDOW_UPDATE`
//! overflow handling (the `MAX_FLOW_CONTROL_WINDOW` guard, at both stream and connection
//! level), strict per-stream and connection recv-window enforcement, and the benign
//! `WINDOW_UPDATE`-on-a-closed-stream case.
//!
//! Flow control causes no §5.1 stream-state transitions, so none of this lives in the pure
//! stream-state machine — it's the driver's flow-control *accounting*, which is exactly
//! where h2 overflow / DoS bugs hide.

use super::fixture::*;
use crate::{
    Body, Headers, HttpConfig, Method, Status,
    h2::{H2ErrorCode, frame::Frame, settings::H2Settings},
};
use futures_lite::io::AsyncRead;
use std::{
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

/// A response body larger than the peer's tiny advertised send window must park when the
/// window is exhausted and resume — delivering the remainder — when the peer credits it
/// with a `WINDOW_UPDATE`. This is the send-window-exhaustion cell the test-gaps memory
/// flagged as wholly untested: without correct parking the driver either stalls or
/// busy-spins, and without correct resume the tail of the body is never framed.
#[test]
fn send_window_exhaustion_parks_then_resumes_on_window_update() {
    use crate::headers::hpack::PseudoHeaders;

    let mut fx = DriverFixture::new_server();
    // Peer grants a 5-byte initial send window; our per-stream send window seeds from it.
    fx.complete_handshake_with_peer_settings(H2Settings::default().with_initial_window_size(5));

    // Body-less request (END_STREAM) so the recv half is already closed — only the send
    // half's window behavior is under test.
    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // 12-byte body, 5-byte window: the pump should frame exactly the first 5 bytes, then
    // park on the exhausted window without sending END_STREAM.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_send(
        1,
        pseudos,
        Headers::new(),
        Some(Body::new_static(b"hello world!" as &[u8])),
    );
    let _ = fx.tick();

    let first = fx.next_outbound_frames();
    assert_eq!(
        data_bytes(&first, 1),
        5,
        "send pump should frame exactly the 5-byte window's worth of DATA, then park; got \
         {first:?}",
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
        "no END_STREAM while the window is exhausted mid-body; got {first:?}",
    );
    assert!(
        fx.connection.streams_lock().contains_key(&1),
        "stream must stay live while parked on a zero send window",
    );

    // Peer opens the window; the pump resumes and frames the remaining 7 bytes + END_STREAM.
    fx.peer_window_update(1, 20);
    let _ = fx.tick();
    let after = fx.next_outbound_frames();
    assert_eq!(
        data_bytes(&after, 1),
        7,
        "after WINDOW_UPDATE the pump should frame the remaining 7 body bytes; got {after:?}",
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
        "resumed send should terminate the stream with END_STREAM; got {after:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "with the body fully sent and recv already closed, the server should remove the stream",
    );
}

/// A peer `WINDOW_UPDATE` that would push a *stream's* send window past `2^31 - 1` is a
/// stream-level `FLOW_CONTROL_ERROR`: the driver RSTs that stream and removes it, leaving
/// the connection running.
#[test]
fn peer_window_update_overflow_resets_stream() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // Window starts at the default 65535; the max single increment (0x7FFF_FFFF) overflows
    // past 2^31 - 1.
    fx.peer_window_update(1, 0x7FFF_FFFF);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::RstStream {
                stream_id: 1,
                error_code: H2ErrorCode::FlowControlError,
            }
        )),
        "stream-window overflow must earn RST_STREAM(FLOW_CONTROL_ERROR); got {frames:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "the overflowed stream should be removed",
    );
    assert!(
        !frames.iter().any(|f| matches!(f, Frame::Goaway { .. })),
        "a stream-level flow-control error must not tear down the connection; got {frames:?}",
    );
}

/// A peer `WINDOW_UPDATE` on stream 0 that would push the *connection* send window past
/// `2^31 - 1` is a connection-level `FLOW_CONTROL_ERROR` → GOAWAY.
#[test]
fn peer_window_update_overflow_on_connection_goaways() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Connection send window starts at 65535; the max increment overflows it.
    fx.peer_window_update(0, 0x7FFF_FFFF);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::Goaway {
                error_code: H2ErrorCode::FlowControlError,
                ..
            }
        )),
        "connection-window overflow must GOAWAY with FLOW_CONTROL_ERROR; got {frames:?}",
    );
}

/// Strict per-stream flow control: a peer that sends past the window we granted on a single
/// stream earns a connection-level `FLOW_CONTROL_ERROR`. Since the `avail + buffered ≤ target`
/// invariant makes the window double as the recv buffer bound, this is also the memory-DoS bound.
/// Uses a tiny configured stream window so the test sends a handful of bytes rather than the
/// 256 KiB default.
#[test]
fn peer_data_past_stream_window_is_connection_error() {
    let mut fx = DriverFixture::new_server_with_config(
        HttpConfig::default()
            .with_h2_initial_stream_window_size(100)
            .with_h2_max_stream_recv_window_size(100),
    );
    fx.complete_handshake();

    // Recv half left open so the DATA is routed. The handler never reads, so the stream stays at
    // its 100-byte initial window — no promotion to the read-target.
    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // 150 bytes against a 100-byte stream window — overruns it.
    fx.peer_data(1, &[0u8; 150], false);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::Goaway {
                error_code: H2ErrorCode::FlowControlError,
                ..
            }
        )),
        "DATA past the per-stream recv window must be a connection FLOW_CONTROL_ERROR; got \
         {frames:?}",
    );
}

/// The connection-level recv window is enforced, not merely tracked: aggregate inbound DATA
/// that overruns it is a connection-level `FLOW_CONTROL_ERROR` → GOAWAY, even when no single
/// stream exceeds the per-stream buffer cap. Pins the connection window to its 65535 floor
/// (the configured target only ever raises it, never lowers) and floods just past that on one
/// stream whose 1 MiB per-stream cap stays untouched — so the connection window is the only
/// bound that can fire.
#[test]
fn peer_data_past_connection_window_is_connection_error() {
    let mut fx = DriverFixture::new_server_with_config(
        HttpConfig::default().with_h2_initial_connection_window_size(65_535),
    );
    fx.complete_handshake();

    // Recv half left open so the DATA routes into the recv ring rather than being rejected.
    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // Four 16384-byte DATA frames total 65536 — one byte past the 65535 connection window, so
    // the fourth tips it negative. Each frame is within the default max frame size, and the
    // running total (≤ 65536) stays under the 1 MiB per-stream cap.
    let chunk = [0u8; 16_384];
    for _ in 0..4 {
        fx.peer_data(1, &chunk, false);
    }

    let mut frames = Vec::new();
    for _ in 0..6 {
        let _ = fx.tick();
        frames.extend(fx.next_outbound_frames());
        if frames.iter().any(|f| matches!(f, Frame::Goaway { .. })) {
            break;
        }
    }
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::Goaway {
                error_code: H2ErrorCode::FlowControlError,
                ..
            }
        )),
        "DATA past the connection recv window must be a connection FLOW_CONTROL_ERROR; got \
         {frames:?}",
    );
}

/// A `WINDOW_UPDATE` arriving on a stream that has already closed is benign — the peer may
/// credit a stream it hasn't yet observed our END_STREAM on (RFC 9113 §5.1). The driver
/// ignores it: no error, no GOAWAY, connection stays running.
#[test]
fn peer_window_update_on_closed_stream_is_ignored() {
    use crate::{h2::acceptor::types::DriverState, headers::hpack::PseudoHeaders};

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Open + fully answer a body-less request so the stream closes and is removed.
    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_send(1, pseudos, Headers::new(), None);
    let _ = fx.tick();
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "body-less response should close + remove the stream",
    );
    let _ = fx.next_outbound_frames();

    // Late WINDOW_UPDATE on the now-closed (but ≤ last_peer_stream_id) stream.
    fx.peer_window_update(1, 100);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        !frames.iter().any(|f| matches!(f, Frame::Goaway { .. })),
        "WINDOW_UPDATE on a closed stream must be ignored, not error the connection; got \
         {frames:?}",
    );
    assert_eq!(
        fx.driver.state,
        DriverState::Running,
        "connection should still be running after a benign late WINDOW_UPDATE",
    );
}

/// The increment of the first `WINDOW_UPDATE` emitted for `stream_id`, if any.
fn stream_window_update(frames: &[Frame], stream_id: u32) -> Option<u32> {
    frames.iter().find_map(|f| match f {
        Frame::WindowUpdate {
            stream_id: id,
            increment,
        } if *id == stream_id => Some(*increment),
        _ => None,
    })
}

/// Drive the handler's first `poll_read` on the request body, latching `recv.is_reading` (the
/// signal the driver promotes the stream window on). With no DATA buffered yet the read parks.
fn poll_request_body_once(conn: &mut crate::Conn<crate::h2::H2Transport>) {
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut buf = [0u8; 16];
    let mut body = conn.request_body();
    assert!(
        Pin::new(&mut body)
            .poll_read(&mut cx, &mut buf)
            .is_pending(),
        "first body poll_read should park (no DATA buffered yet)",
    );
}

/// Lazy two-tier promotion: once the handler signals intent to read the body (`is_reading`), the
/// driver grows the stream window from the advertised initial to the read-target in a single
/// `WINDOW_UPDATE` carrying exactly the delta (`max - initial`).
#[test]
fn is_reading_promotes_stream_window_by_the_delta() {
    let mut fx = DriverFixture::new_server_with_config(
        HttpConfig::default()
            .with_h2_initial_stream_window_size(100)
            .with_h2_max_stream_recv_window_size(1000),
    );
    fx.complete_handshake();
    // Clear the handshake frames (initial SETTINGS + connection-level WINDOW_UPDATE).
    let _ = fx.next_outbound_frames();

    // Recv half open so the handler can read the body.
    fx.peer_open_stream(1, Method::Post, "/", false);
    let mut conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };
    // Opening a stream alone grants no stream-level window update.
    assert_eq!(
        stream_window_update(&fx.next_outbound_frames(), 1),
        None,
        "an unread stream should not be credited before the handler reads",
    );

    poll_request_body_once(&mut conn);
    let _ = fx.tick();

    assert_eq!(
        stream_window_update(&fx.next_outbound_frames(), 1),
        Some(900),
        "promotion on is_reading should emit WINDOW_UPDATE of exactly max - initial (1000 - 100)",
    );
}

/// A stream whose handler never reads the body stays at the modest advertised initial — no
/// promotion, no stream-level `WINDOW_UPDATE`. This is the recv-prioritization property: the
/// connection's shared window isn't spent crediting streams the application is ignoring.
#[test]
fn unread_stream_is_never_promoted() {
    let mut fx = DriverFixture::new_server_with_config(
        HttpConfig::default()
            .with_h2_initial_stream_window_size(100)
            .with_h2_max_stream_recv_window_size(1000),
    );
    fx.complete_handshake();
    let _ = fx.next_outbound_frames();

    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // Several ticks with the handler never touching the body: no stream-1 WINDOW_UPDATE.
    for _ in 0..3 {
        let _ = fx.tick();
        assert_eq!(
            stream_window_update(&fx.next_outbound_frames(), 1),
            None,
            "an unread stream must never be promoted to the read-target",
        );
    }
}
