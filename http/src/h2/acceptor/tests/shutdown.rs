use super::fixture::*;
use crate::{
    Body, Headers, Method, Status,
    h2::{
        H2ErrorCode,
        acceptor::types::{CloseOutcome, DriverState},
        frame::Frame,
    },
};
use std::task::Poll;

/// Closing → Drained is gated on the in-flight stream predicate: while any stream has an
/// active send cursor or unfinished recv side, the driver stays in Closing — only once
/// both clear can it transition to Drained. Validates the behavior the wip-commit
/// docstring promises:
///
/// > Defer the transition while in-flight streams still have outbound (SendCursor not yet
/// > Complete) OR inbound (`recv.eof` not yet set) work.
///
/// Wire-level assertion: after begin_close with an in-flight stream open, no FIN-style
/// close happens (state stays Closing); after peer ends the stream's recv side, the
/// transition fires.
#[test]
fn closing_to_drained_waits_for_in_flight_stream() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Open stream 1 with `end_stream=false` so the recv side stays in-flight after the
    // request HEADERS — `has_pending_recv` will be true until peer END_STREAM lands.
    fx.peer_open_stream(1, Method::Post, "/", false);
    let conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };
    // Hold the conn for the duration of the test — dropping it would tear down the
    // H2Transport and let the stream complete via a different path than the one we're
    // exercising here.
    let _conn_guard = conn;

    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Closing,
        "in-flight stream's open recv side should hold the driver in Closing",
    );

    // Peer closes its half of stream 1. Driver's recv pump (still running in Closing per
    // the wip commit) picks up END_STREAM, recv.eof flips, predicate clears, transition
    // fires.
    fx.peer_data(1, &[], true);
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Drained,
        "with the last in-flight stream's recv side closed, Closing should advance to Drained",
    );
}

/// The send pump runs in `Closing` (not just `Running`): once we've begun closing, any
/// stream with a staged submission must still be framed and put on the wire — gRPC and
/// other late-trailer patterns submit the response right around the same time the
/// shutdown decision fires, and dropping the in-flight response would be a regression.
/// The wip-commit changed the send-pump's run condition from `Running` to
/// `Running | Closing` for this reason; this test pins it.
#[test]
fn send_pump_emits_response_in_closing() {
    use crate::headers::hpack::PseudoHeaders;

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // Stage a small response submission, then immediately begin_close — the send pump
    // hasn't picked it up yet, so the question is whether the pump runs in Closing.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let body = Body::new_static(b"hi" as &[u8]);
    let _submit = fx
        .connection
        .submit_send(1, pseudos, Headers::new(), Some(body));
    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    let response_headers = frames
        .iter()
        .filter(|f| matches!(f, Frame::Headers { stream_id: 1, .. }))
        .count();
    let data_frames = frames
        .iter()
        .filter(|f| matches!(f, Frame::Data { stream_id: 1, .. }))
        .count();
    assert!(
        response_headers >= 1,
        "send pump should emit response HEADERS for stream 1 while Closing; got {frames:?}",
    );
    assert!(
        data_frames >= 1,
        "send pump should emit DATA for stream 1 while Closing; got {frames:?}",
    );
    let end_stream_data = frames.iter().any(|f| {
        matches!(
            f,
            Frame::Data {
                stream_id: 1,
                end_stream: true,
                ..
            }
        )
    });
    assert!(
        end_stream_data,
        "send pump should terminate stream 1 with END_STREAM; got {frames:?}",
    );
}

/// The recv pump runs in `Closing` (not just `Running`): trailing HEADERS the peer sends
/// after the driver has begun closing must still be decoded and stashed on the in-flight
/// stream's `recv.trailers` slot — otherwise gRPC trailers can vanish under shutdown
/// pressure. The wip-commit changed the read-side pump's run condition from
/// `Running` to `Running | Closing` for precisely this reason; this test pins the
/// behavior so the lifecycle refactor preserves it.
#[test]
fn recv_pump_decodes_trailing_headers_in_closing() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // POST with end_stream=false leaves the request body open — we'll send trailing
    // HEADERS as the terminator instead of DATA(END_STREAM).
    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };
    let state = fx
        .connection
        .streams_lock()
        .get(&1)
        .cloned()
        .expect("stream 1 registered");

    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();
    assert_eq!(fx.driver.state, DriverState::Closing);

    // Trailing HEADERS arrive *after* our GOAWAY went out. The recv-pump-in-Closing rule
    // says we keep decoding for streams already in flight.
    let mut trailers_in = Headers::new();
    trailers_in.insert("grpc-status", "0");
    trailers_in.insert("grpc-message", "ok");
    fx.peer_trailers(1, &trailers_in);
    let _ = fx.tick();

    let stashed = state
        .recv
        .trailers
        .lock()
        .expect("recv.trailers mutex poisoned")
        .clone()
        .expect("driver should have stashed trailers from the post-GOAWAY frame");
    assert_eq!(stashed.get_str("grpc-status"), Some("0"));
    assert_eq!(stashed.get_str("grpc-message"), Some("ok"));
}

/// A peer HEADERS opening a *new* stream while the driver is in `Closing` must not be
/// yielded as a `Conn` — once we've sent GOAWAY, the peer shouldn't be opening new
/// streams, and even if it does we mustn't dispatch a handler for one we're about to tear
/// down. Pairs with [`closing_to_drained_waits_for_in_flight_stream`] above (which keeps
/// the driver in Closing long enough to observe this branch).
#[test]
fn closing_discards_new_stream_headers() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Keep the driver in Closing by holding an in-flight stream with an open recv side.
    fx.peer_open_stream(1, Method::Post, "/", false);
    let stream_one = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();
    assert_eq!(fx.driver.state, DriverState::Closing);
    let _ = fx.next_outbound_bytes();

    // Peer (misbehaving) opens a new stream past the GOAWAY.
    fx.peer_open_stream(3, Method::Get, "/late", true);
    let polled = fx.tick();
    assert!(
        !matches!(polled, Poll::Ready(Some(Ok(_)))),
        "post-GOAWAY HEADERS opening a new stream must not yield a Conn; got {polled:?}",
    );

    // Cleanup: drop the held stream-1 conn so its Drop doesn't outlive the fixture and
    // accidentally interleave assertions in a later test (unimportant for correctness;
    // makes the test scope explicit).
    drop(stream_one);
}

/// `begin_close` is idempotent: a second call once the driver is already `Closing` (or
/// `Drained`) does not queue another GOAWAY and does not overwrite the prior close
/// outcome. The peer-mirror case in the wild — peer GOAWAY arrives after we've already
/// begun closing — would otherwise ping-pong, each side re-arming on the other's frame.
///
/// Asserts at the wire level (count of GOAWAY frames in outbound bytes) so the
/// future lifecycle-enum refactor doesn't change what this test exercises.
#[test]
fn begin_close_is_idempotent() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();
    assert_eq!(fx.driver.state, DriverState::Running);

    // First close: graceful. Drains outbound to put the GOAWAY on the wire.
    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();
    assert_eq!(fx.driver.state, DriverState::Drained);
    let first_round = fx.next_outbound_frames();
    assert_eq!(
        count_goaways(&first_round),
        1,
        "graceful begin_close should emit exactly one GOAWAY; got {first_round:?}",
    );
    let first_goaway_code = first_round.iter().find_map(|f| match f {
        Frame::Goaway { error_code, .. } => Some(*error_code),
        _ => None,
    });
    assert_eq!(
        first_goaway_code,
        Some(H2ErrorCode::NoError),
        "graceful close should queue NoError, got {first_goaway_code:?}",
    );

    // Second close: protocol error. Must be a no-op — no fresh GOAWAY, state unchanged.
    fx.driver
        .begin_close(CloseOutcome::Protocol(H2ErrorCode::InternalError));
    let _ = fx.tick();
    let second_round = fx.next_outbound_frames();
    assert_eq!(
        count_goaways(&second_round),
        0,
        "second begin_close after Closing/Drained must not re-queue GOAWAY; got {second_round:?}",
    );
}

/// A peer `RST_STREAM` clears the `Closing → Drained` in-flight gate. Companion to
/// [`closing_to_drained_waits_for_in_flight_stream`], which clears the gate via the peer's
/// END_STREAM; here a peer reset removes the last in-flight stream, so the next tick
/// advances to Drained.
#[test]
fn peer_rst_clears_closing_drain_gate() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Post, "/", false);
    // Held for the test so its Drop doesn't complete the stream by a different path.
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Closing,
        "in-flight stream's open recv side should hold the driver in Closing",
    );

    fx.peer_rst_stream(1, H2ErrorCode::Cancel);
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Drained,
        "peer RST removing the last in-flight stream should let Closing advance to Drained",
    );
}
