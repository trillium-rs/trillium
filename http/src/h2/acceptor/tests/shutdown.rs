use super::fixture::*;
use crate::{
    Body, Headers, Method, Status,
    h2::{
        H2ErrorCode,
        acceptor::types::{CloseOutcome, DriverState},
        frame::Frame,
        settings::H2Settings,
    },
    headers::hpack::PseudoHeaders,
};
use std::task::Poll;

/// Count `RST_STREAM` frames for `stream_id` in a decoded frame list.
fn count_rst(frames: &[Frame], stream_id: u32) -> usize {
    frames
        .iter()
        .filter(|f| matches!(f, Frame::RstStream { stream_id: id, .. } if *id == stream_id))
        .count()
}

/// Closing → Drained is gated on the in-flight stream predicate: the driver stays in Closing
/// while any stream has an active send cursor, an *open send half* (a handler that hasn't
/// responded yet — half-closed-remote is not drained), or an unfinished recv side. Only when a
/// stream is fully closed does it stop holding the gate.
///
/// In particular, a peer closing its *recv* side is **not** enough on its own: the handler
/// still owes a response, so the send half stays open and the driver must keep polling. (This is
/// the `h2-shutdown-drain-deadlock` regression — finishing here would orphan the response the
/// handler is about to submit.) Draining only fires once the stream's send half also closes —
/// here, when the handler gives up and its dropped transport sends `RST_STREAM`.
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

    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Closing,
        "in-flight stream's open recv side should hold the driver in Closing",
    );

    // Peer closes its half of stream 1 (recv side now done) — but the handler hasn't responded,
    // so the send half is still open. The driver must *not* drain yet.
    fx.peer_data(1, &[], true);
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Closing,
        "recv-closed alone must not drain: the handler still owes a response (send half open)",
    );

    // The handler gives up: dropping the Conn tears down the H2Transport → RST_STREAM(Cancel),
    // closing the send half. Now the stream is fully closed and the gate clears.
    drop(conn);
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Drained,
        "with the in-flight stream fully closed (recv + send), Closing should advance to Drained",
    );
}

/// Regression for the orphaned-handler swansong-guard leak (surfaced by the h2spec conformance
/// runner hanging on `shut_down()`).
///
/// When a connection dies mid-flight (i/o error / peer FIN — the dominant conformance trigger,
/// "unexpected end of file") the driver's teardown wakes any handler parked reading the request
/// body. That handler then runs to completion and submits its response — *into a connection whose
/// driver has already exited*. Nothing will ever frame that submission. `SubmitSend` must observe
/// the stream's terminal (reset) state and resolve with an error instead of parking forever; a
/// parked handler holds its swansong guard, so graceful shutdown never completes.
///
/// This reproduces the exact ordering: kill the connection first, *then* submit (the handler only
/// wakes and responds after the recv fan-out).
#[test]
fn submit_after_connection_death_resolves_instead_of_hanging() {
    use std::{
        future::Future,
        task::{Context, Poll as TaskPoll},
    };

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Open a request stream with an open body (no END_STREAM): a real handler parks reading it.
    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // The connection dies: the peer closes the transport, the driver reads EOF and finishes with
    // an i/o error. (`run_h2` would now break and drop the driver.)
    fx.peer.close();
    match fx.tick() {
        Poll::Ready(Some(Err(_))) => {}
        other => panic!("expected the driver to finish with an i/o error, got {other:?}"),
    }

    // The handler wakes (recv EOF), produces a response, and submits — but the driver is gone.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let submit = fx.connection.submit_send(
        1,
        pseudos,
        Headers::new(),
        Some(Body::new_static(b"ok" as &[u8])),
    );

    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut submit = Box::pin(submit);
    assert!(
        matches!(submit.as_mut().poll(&mut cx), TaskPoll::Ready(Err(_))),
        "SubmitSend on a connection whose driver has exited must resolve with an error, not park \
         forever — a parked handler holds its swansong guard and hangs graceful shutdown",
    );
}

/// Companion to [`submit_after_connection_death_resolves_instead_of_hanging`] for the *other*
/// ordering: the handler has already submitted and is parked in `SubmitSend` (response staged,
/// awaiting the driver to frame it) when the connection dies. The connection-death fan-out must
/// wake the send-completion waiter — the recv-side wakes don't reach it — so a real executor
/// re-polls and the future resolves. A zero peer send window keeps the staged body from being
/// framed, so the only thing that can wake the parked submit is the teardown.
#[test]
fn connection_death_wakes_parked_submit() {
    use std::{
        future::Future,
        task::{Context, Poll as TaskPoll},
    };

    let mut fx = DriverFixture::new_server();
    // Zero peer send window: a staged response body cannot be framed, so `submit_resolved`
    // never flips on its own — the submit stays parked until the connection dies.
    fx.complete_handshake_with_peer_settings(H2Settings::default().with_initial_window_size(0));

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let submit = fx.connection.submit_send(
        1,
        pseudos,
        Headers::new(),
        Some(Body::new_static(b"stalled-body" as &[u8])),
    );
    let _ = fx.tick(); // driver picks up the submission; HEADERS may frame, body stalls on window 0.

    let (counting, waker) = counting_waker();
    let mut cx = Context::from_waker(&waker);
    let mut submit = Box::pin(submit);
    assert!(
        matches!(submit.as_mut().poll(&mut cx), TaskPoll::Pending),
        "window-stalled submit should be Pending before the connection dies",
    );
    assert_eq!(counting.count(), 0, "no wake yet");

    // Connection dies. The teardown must wake the parked submit's completion waker.
    fx.peer.close();
    match fx.tick() {
        Poll::Ready(Some(Err(_))) => {}
        other => panic!("expected the driver to finish with an i/o error, got {other:?}"),
    }
    assert!(
        counting.count() >= 1,
        "connection death must wake a parked SubmitSend's completion waker",
    );
    assert!(
        matches!(submit.as_mut().poll(&mut cx), TaskPoll::Ready(Err(_))),
        "after the connection dies the parked submit must resolve with an error",
    );
}

/// Regression for the second orphaned-handler leak the conformance runner surfaced (h2spec
/// `http2/6.9`, `http2/8.1`): a single-stream teardown (peer `RST_STREAM`, a flow-control
/// overflow, a malformed trailing HEADERS — anything routed through `complete_and_remove_stream`)
/// must wake a handler parked reading the request body. The teardown chokepoint (`signal_close`)
/// previously woke only the response-headers waiter, not the recv waiter, so a body-reading
/// handler parked on a stream torn down this way never observed EOF and leaked its swansong guard.
///
/// Uses a peer `RST_STREAM` as the trigger — the simplest of the family, and the one whose path
/// no longer wakes the recv waiter explicitly (it now relies on `signal_close`).
#[test]
fn stream_reset_wakes_handler_parked_reading_body() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Open with an open body (no END_STREAM): a real handler parks reading it.
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

    // Simulate the handler parked in a body `poll_read`: a waker registered on recv.waker.
    let (counting, waker) = counting_waker();
    state.recv.waker.register(&waker);

    // Peer resets the stream — the driver moves it to `Closed{Reset}` and removes it.
    fx.peer_rst_stream(1, H2ErrorCode::Cancel);
    let _ = fx.tick();

    assert!(
        state.lifecycle_lock().recv_closed(),
        "a reset stream must report recv-closed so the body read sees EOF",
    );
    assert!(
        counting.count() >= 1,
        "tearing the stream down must wake a handler parked reading the request body — otherwise \
         it never observes EOF and leaks its swansong guard",
    );
}

/// Companion to [`stream_reset_wakes_handler_parked_reading_body`] for the *other* ordering — the
/// actual h2spec `http2/6.9` leak. A driver-initiated stream RST (here a peer `WINDOW_UPDATE`
/// overflowing the stream send window) must leave the stream's protocol state terminal, so a
/// handler that polls the request body *after* the reset sees EOF rather than parking. The leak
/// was a handler task spawned but not yet polled when the overflow RST fired: the reset removed
/// the stream and emitted RST_STREAM but never transitioned the lifecycle, so the handler's first
/// `poll_read` saw an open stream, parked on a waker nothing would ever fire, and leaked its guard.
#[test]
fn flow_control_rst_leaves_stream_terminal() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

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

    // Peer overflows the stream send window (default 65535 + 2^31-1) → stream-level RST + removal.
    fx.peer_window_update(1, i32::MAX as u32);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert_eq!(
        count_rst(&frames, 1),
        1,
        "a stream send-window overflow must emit a stream-level RST_STREAM; got {frames:?}",
    );
    assert!(
        state.lifecycle_lock().recv_closed(),
        "a flow-control RST must leave the stream recv-closed so a handler polling the body after \
         the reset sees EOF instead of parking forever (the http2/6.9 guard leak)",
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

/// Regression probe
///
/// The existing drain tests ([`closing_to_drained_waits_for_in_flight_stream`],
/// [`peer_rst_clears_closing_drain_gate`]) all clear the gate with an *inbound* peer frame.
/// The shutdown deadlock is the case where no inbound frame is coming: the handler itself
/// abandons the stream mid-flight (drops the `Conn`), and the gate must clear from that
/// local act alone.
///
/// Here the handler drops an in-flight (recv-open, no response submitted) stream while the
/// driver is `Closing`. `H2Transport::drop` calls `request_reset(Cancel)`, which clears the
/// send queue and stages a preempting `Reset`; the send pump must frame it (RST on the wire,
/// so the peer learns) and remove the stream, letting `Closing → Drained` fire — all without
/// the peer sending anything.
#[test]
fn handler_drop_during_closing_resets_and_drains_without_peer_frame() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Post, "/", false);
    let conn = match fx.tick() {
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
    let _ = fx.next_outbound_bytes();

    // Handler gives up: dropping the Conn tears down the H2Transport → request_reset(Cancel).
    // No peer frame follows.
    drop(conn);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert_eq!(
        count_rst(&frames, 1),
        1,
        "a handler-dropped stream must emit RST_STREAM so the peer learns; got {frames:?}",
    );
    assert_eq!(
        fx.driver.state,
        DriverState::Drained,
        "a locally-abandoned stream must clear the drain gate without an inbound peer frame",
    );
}

/// Isolates the gate's dependence on an inbound frame for a send the driver *cannot complete
/// on its own*. Peer grants a zero send window, the handler submits a response body, then
/// shutdown begins. The body can't be framed (no window), so `has_active_send_cursors` holds
/// the driver in `Closing`. This is the `cursor_present=true` blocker from the bug's pass
/// trace. The only thing that can advance it is an inbound peer frame:
///
/// 1. with the Conn held and no peer frame, the driver is stuck in `Closing`;
/// 2. a peer `WINDOW_UPDATE` (inbound) unblocks the body → terminator → `Drained`.
///
/// In a reset-race deadlock the peer has already torn down and that releasing frame never
/// arrives — this test pins the mechanism so the fix can make the gate clearable from local
/// state instead.
#[test]
fn window_stalled_send_holds_closing_until_inbound_frame() {
    let mut fx = DriverFixture::new_server();
    // Zero initial send window: the response body cannot be framed at all.
    fx.complete_handshake_with_peer_settings(H2Settings::default().with_initial_window_size(0));

    // Body-less request (END_STREAM) → recv half already closed, so `has_pending_recv` is
    // out of the picture and the *only* possible blocker is the send cursor.
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
        Some(Body::new_static(b"hello" as &[u8])),
    );
    let _ = fx.tick();

    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Closing,
        "a window-stalled in-flight send should hold the driver in Closing",
    );
    // Tick again with no inbound frame: still stuck — nothing local can advance it.
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Closing,
        "with no inbound frame, the window-stalled send keeps the gate closed",
    );

    // The releasing event is inbound: the peer opens the window, the body frames, the stream
    // terminates, and the gate clears.
    fx.peer_window_update(1, 100);
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Drained,
        "an inbound WINDOW_UPDATE should unblock the send and let Closing advance to Drained",
    );
}

/// Deterministic unit-level guard for `h2-shutdown-drain-deadlock`: a `HalfClosedRemote` stream
/// (full request received, no response yet) must hold `Closing` until its send half closes.
///
/// This is the live `h2_shutdown_drain` integration test reduced to its essential state machine,
/// with no timing: the bug was the driver draining + finishing while a handler still owed a
/// response, orphaning the response `SubmitSend`. Here the request arrives with `END_STREAM`
/// (recv closed → `HalfClosedRemote`) and no response is submitted, so `has_pending_recv` and
/// `has_active_send_cursors` are both false — only `has_open_send_half` keeps the gate shut. We
/// then submit the response and confirm the driver frames it and only *then* drains.
#[test]
fn half_closed_remote_holds_closing_until_response_sent() {
    use crate::headers::hpack::PseudoHeaders;

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Body-less GET: request HEADERS carry END_STREAM → recv half closed at once
    // (`HalfClosedRemote`). The handler hasn't responded, so the send half is still open.
    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    fx.driver.begin_close(CloseOutcome::Graceful);
    let _ = fx.tick();
    assert_eq!(
        fx.driver.state,
        DriverState::Closing,
        "a half-closed-remote stream with no response yet must hold the driver in Closing — \
         draining here would orphan the response the handler is about to submit",
    );
    // Several more ticks with no inbound frame: it must stay put (nothing local drains a stream
    // whose handler still owes a response).
    for _ in 0..3 {
        let _ = fx.tick();
        assert_eq!(fx.driver.state, DriverState::Closing);
    }

    // Handler responds. The send pump (running in Closing) frames it, the send half closes, and
    // only now does the gate clear.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_send(
        1,
        pseudos,
        Headers::new(),
        Some(Body::new_static(b"ok" as &[u8])),
    );
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        frames
            .iter()
            .any(|f| matches!(f, Frame::Headers { stream_id: 1, .. })),
        "the response HEADERS must be framed while Closing, not dropped; got {frames:?}",
    );
    assert_eq!(
        fx.driver.state,
        DriverState::Drained,
        "once the response is sent (send half closed), Closing should advance to Drained",
    );
}
