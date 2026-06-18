//! RFC 9218 priority-scheduling tests for the send pump. These drive several concurrent
//! response streams through the `DriverFixture` and assert on the *order* and *share* of the
//! DATA frames the pump emits — the wire-observable consequence of
//! [`advance_outbound_sends`][super::super::send]'s two-pass priority scheduler.
//!
//! The default connection *send* window is 65535 (raised only by a peer `WINDOW_UPDATE(0)`,
//! which the fixture never sends unless a test asks), so a body larger than that makes the
//! connection window the binding constraint — letting these tests observe exactly how the
//! pump divides one window's worth of bandwidth across competing streams.

use super::fixture::*;
use crate::{
    Body, Conn, Headers, Method, Priority, Status,
    h2::{H2Error, H2Transport, frame::Frame},
    headers::hpack::PseudoHeaders,
};
use std::task::Poll;

/// Sum the DATA payload bytes a frame batch carries for `stream_id`.
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

/// The stream ids of the DATA frames in `frames`, in wire order — the sequence that reveals
/// run-to-completion (`[1, 1, …, 3, 3]`) versus round-robin (`[1, 3, 1, 3]`) interleaving.
fn data_stream_order(frames: &[Frame]) -> Vec<u32> {
    frames
        .iter()
        .filter_map(|f| match f {
            Frame::Data { stream_id, .. } => Some(*stream_id),
            _ => None,
        })
        .collect()
}

/// Whether `frames` carries a HEADERS block for `stream_id`.
fn has_headers(frames: &[Frame], stream_id: u32) -> bool {
    frames
        .iter()
        .any(|f| matches!(f, Frame::Headers { stream_id: id, .. } if *id == stream_id))
}

/// Unwrap a yielded request `Conn`. Callers bind it for the test's lifetime: dropping the
/// `Conn` drops its `H2Transport`, which would reset the still-open stream out from under the
/// response the test submits via [`H2Connection::submit_send`][crate::h2::H2Connection].
fn expect_conn(polled: Poll<Option<Result<Conn<H2Transport>, H2Error>>>) -> Conn<H2Transport> {
    match polled {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected a yielded Conn, got {other:?}"),
    }
}

fn ok() -> PseudoHeaders<'static> {
    PseudoHeaders::default().with_status(Status::Ok)
}

/// A higher-urgency response drains to completion before a lower-urgency one gets any
/// bandwidth, even when the urgent stream has the *higher* id (so id order alone would
/// schedule it second). With both bodies exceeding the connection window, the urgent stream
/// consumes the entire window and the other sends zero DATA — the core "more urgent finishes
/// first" behavior of run-to-completion scheduling.
#[test]
fn higher_urgency_drains_before_lower() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Stream 1 is the *less* urgent (u=5); stream 3 the *more* urgent (u=1).
    fx.peer_open_stream_with_priority(1, "/low", Priority::new(5), true);
    let _c1 = expect_conn(fx.tick());
    fx.peer_open_stream_with_priority(3, "/high", Priority::new(1), true);
    let _c3 = expect_conn(fx.tick());

    let _ = fx.connection.submit_send(
        1,
        ok(),
        Headers::new(),
        Some(Body::new_static(vec![b'a'; 70_000])),
    );
    let _ = fx.connection.submit_send(
        3,
        ok(),
        Headers::new(),
        Some(Body::new_static(vec![b'b'; 70_000])),
    );

    let _ = fx.tick();
    let frames = fx.next_outbound_frames();

    assert_eq!(
        data_bytes(&frames, 3),
        65_535,
        "the more-urgent stream 3 should consume the entire connection window; got {frames:?}",
    );
    assert_eq!(
        data_bytes(&frames, 1),
        0,
        "the less-urgent stream 1 should get no bandwidth until the urgent stream stalls; got \
         {frames:?}",
    );
    assert_eq!(
        data_stream_order(&frames).first(),
        Some(&3),
        "the first DATA frame must belong to the more-urgent stream; got {frames:?}",
    );
}

/// Two non-incremental responses at the *same* urgency are served sequentially: the first
/// (lower id) drains to completion — including its `END_STREAM` — before the second sends a
/// byte. A small first body lets it finish inside the window, so the same tick then starts the
/// second; the DATA order is all-of-1 then all-of-3, never interleaved.
#[test]
fn same_urgency_non_incremental_is_sequential() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream_with_priority(1, "/a", Priority::new(3), true);
    let _c1 = expect_conn(fx.tick());
    fx.peer_open_stream_with_priority(3, "/b", Priority::new(3), true);
    let _c3 = expect_conn(fx.tick());

    // Stream 1's 20000-byte body fits inside the 65535 window with room to spare, so it
    // completes and stream 3 picks up the remaining 45535 of the window.
    let _ = fx.connection.submit_send(
        1,
        ok(),
        Headers::new(),
        Some(Body::new_static(vec![b'a'; 20_000])),
    );
    let _ = fx.connection.submit_send(
        3,
        ok(),
        Headers::new(),
        Some(Body::new_static(vec![b'b'; 70_000])),
    );

    let _ = fx.tick();
    let frames = fx.next_outbound_frames();

    assert_eq!(
        data_bytes(&frames, 1),
        20_000,
        "stream 1 should drain its whole body before stream 3 starts; got {frames:?}",
    );
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::Data {
                stream_id: 1,
                end_stream: true,
                ..
            }
        )),
        "stream 1 should finish (END_STREAM) before stream 3 sends; got {frames:?}",
    );
    assert_eq!(
        data_bytes(&frames, 3),
        45_535,
        "stream 3 should get the remainder of the window after stream 1 completes; got {frames:?}",
    );

    let order = data_stream_order(&frames);
    let first_three = order
        .iter()
        .position(|&id| id == 3)
        .expect("stream 3 sent DATA");
    assert!(
        order[..first_three].iter().all(|&id| id == 1),
        "all of stream 1's DATA must precede any of stream 3's (sequential, not interleaved); got \
         order {order:?}",
    );
}

/// Two incremental responses at the same urgency share the window frame-by-frame: the DATA
/// order alternates between them and each receives roughly half the window — RFC 9218's `i=?1`
/// interleaving, the deliberate contrast to the non-incremental sequential case above.
#[test]
fn same_urgency_incremental_round_robins() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    let incremental = Priority::new(3).with_incremental(true);
    fx.peer_open_stream_with_priority(1, "/a", incremental, true);
    let _c1 = expect_conn(fx.tick());
    fx.peer_open_stream_with_priority(3, "/b", incremental, true);
    let _c3 = expect_conn(fx.tick());

    // Bodies larger than half the window so neither drains within it.
    let _ = fx.connection.submit_send(
        1,
        ok(),
        Headers::new(),
        Some(Body::new_static(vec![b'a'; 40_000])),
    );
    let _ = fx.connection.submit_send(
        3,
        ok(),
        Headers::new(),
        Some(Body::new_static(vec![b'b'; 40_000])),
    );

    let _ = fx.tick();
    let frames = fx.next_outbound_frames();

    assert_eq!(
        data_stream_order(&frames),
        vec![1, 3, 1, 3],
        "equal-urgency incremental streams should round-robin one frame each; got {frames:?}",
    );
    // 65535 split into 16384-byte frames: 1 gets two (32768), 3 gets one full + the 16383 tail.
    assert_eq!(
        data_bytes(&frames, 1),
        32_768,
        "stream 1 share; got {frames:?}"
    );
    assert_eq!(
        data_bytes(&frames, 3),
        32_767,
        "stream 3 share; got {frames:?}"
    );
}

/// "Don't leave bandwidth idle": when the most-urgent stream stalls on *its own* per-stream send
/// window, the pump falls through to a lower-priority stream rather than leaving the connection
/// idle. With
/// a tight per-stream window but a roomy connection window, the urgent stream emits exactly its
/// per-stream window's worth and then yields; the lower-priority stream gets to send the rest.
#[test]
fn per_stream_window_stall_falls_through_to_lower_priority() {
    let mut fx = DriverFixture::new_server();
    // 16384 per-stream send window; the 65535 connection window stays generous by comparison.
    fx.complete_handshake_with_peer_settings(
        crate::h2::settings::H2Settings::default().with_initial_window_size(16_384),
    );

    fx.peer_open_stream_with_priority(1, "/high", Priority::new(1), true);
    let _c1 = expect_conn(fx.tick());
    fx.peer_open_stream_with_priority(3, "/low", Priority::new(5), true);
    let _c3 = expect_conn(fx.tick());

    let _ = fx.connection.submit_send(
        1,
        ok(),
        Headers::new(),
        Some(Body::new_static(vec![b'a'; 40_000])),
    );
    let _ = fx.connection.submit_send(
        3,
        ok(),
        Headers::new(),
        Some(Body::new_static(vec![b'b'; 40_000])),
    );

    let _ = fx.tick();
    let frames = fx.next_outbound_frames();

    assert_eq!(
        data_bytes(&frames, 1),
        16_384,
        "the urgent stream should send exactly its per-stream window, then stall; got {frames:?}",
    );
    assert_eq!(
        data_bytes(&frames, 3),
        16_384,
        "the lower-priority stream should still send (fall-through past the stalled urgent \
         stream) rather than the connection sitting idle; got {frames:?}",
    );
}

/// Response-start HEADERS are not starved behind an urgent body. Pass 1 emits every stream's
/// HEADERS (they consume no flow-control window) before the priority body pass, so a
/// lower-priority response's start reaches the wire even while a higher-priority stream
/// monopolizes all of the body bandwidth.
#[test]
fn headers_emit_ahead_of_a_monopolizing_body() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream_with_priority(1, "/low", Priority::new(7), true);
    let _c1 = expect_conn(fx.tick());
    fx.peer_open_stream_with_priority(3, "/high", Priority::new(0), true);
    let _c3 = expect_conn(fx.tick());

    // Stream 3 (most urgent) has a body that exceeds the whole connection window; stream 1
    // (least urgent) also has a body it won't get to send this tick.
    let _ = fx.connection.submit_send(
        1,
        ok(),
        Headers::new(),
        Some(Body::new_static(vec![b'a'; 70_000])),
    );
    let _ = fx.connection.submit_send(
        3,
        ok(),
        Headers::new(),
        Some(Body::new_static(vec![b'b'; 70_000])),
    );

    let _ = fx.tick();
    let frames = fx.next_outbound_frames();

    assert!(
        has_headers(&frames, 1) && has_headers(&frames, 3),
        "both responses' HEADERS should be framed regardless of body priority; got {frames:?}",
    );
    assert_eq!(
        data_bytes(&frames, 1),
        0,
        "stream 1's body is still starved behind the urgent stream — only its HEADERS went out; \
         got {frames:?}",
    );
    assert_eq!(
        data_bytes(&frames, 3),
        65_535,
        "stream 3 still consumes the whole window for its body; got {frames:?}",
    );
}

/// A `PRIORITY_UPDATE` reshuffles scheduling mid-flight. Two equal-priority streams start with
/// the lower id (1) draining first; after a `PRIORITY_UPDATE` raises stream 3 to the most
/// urgent level and the connection window is refilled, stream 3 now drains ahead of stream 1's
/// remainder — the reverse of the id order, driven entirely by the update.
#[test]
fn priority_update_reshuffles_scheduling() {
    let mut fx = DriverFixture::new_server();
    // Roomy per-stream windows so only the connection window (and priority) decide ordering.
    fx.complete_handshake_with_peer_settings(
        crate::h2::settings::H2Settings::default().with_initial_window_size(200_000),
    );

    fx.peer_open_stream(1, Method::Get, "/a", true);
    let _c1 = expect_conn(fx.tick());
    fx.peer_open_stream(3, Method::Get, "/b", true);
    let _c3 = expect_conn(fx.tick());

    let _ = fx.connection.submit_send(
        1,
        ok(),
        Headers::new(),
        Some(Body::new_static(vec![b'a'; 70_000])),
    );
    let _ = fx.connection.submit_send(
        3,
        ok(),
        Headers::new(),
        Some(Body::new_static(vec![b'b'; 70_000])),
    );

    // First tick: equal priority, so the lower id (1) drains the whole window first.
    let _ = fx.tick();
    let first = fx.next_outbound_frames();
    assert_eq!(
        data_bytes(&first, 1),
        65_535,
        "before reprioritization the lower-id stream drains first; got {first:?}",
    );
    assert_eq!(
        data_bytes(&first, 3),
        0,
        "stream 3 waits its turn; got {first:?}"
    );

    // Reprioritize stream 3 to the most urgent level (processed before any sends resume since
    // the connection window is still exhausted).
    fx.peer_priority_update(3, Priority::new(0));
    let _ = fx.tick();
    assert!(
        fx.next_outbound_frames().is_empty(),
        "no bandwidth to use yet — the connection window is still exhausted",
    );

    // Refill the connection window; the pump now honors the updated priority.
    fx.peer_window_update(0, 200_000);
    let _ = fx.tick();
    let second = fx.next_outbound_frames();

    let order = data_stream_order(&second);
    let first_one = order.iter().position(|&id| id == 1).unwrap_or(order.len());
    assert!(
        !order.is_empty() && order[..first_one].iter().all(|&id| id == 3),
        "after the PRIORITY_UPDATE, the now-urgent stream 3 should drain ahead of stream 1's \
         remainder; got order {order:?}",
    );
    assert_eq!(
        data_bytes(&second, 3),
        70_000,
        "stream 3 should finish first now that it is the most urgent; got {second:?}",
    );
}
