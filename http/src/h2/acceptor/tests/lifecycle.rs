use super::fixture::*;
use crate::{
    Headers, Method, Status,
    h2::{H2ErrorCode, frame::Frame},
};
use futures_lite::AsyncWrite;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

/// Trailers staged via `submit_trailers` while a `SendCursor` is parked in `Body` phase
/// (waiting on the upgrade outbound buffer to fill) must still reach the wire as a
/// trailing HEADERS frame on the next driver tick. This is the trailers-stranding
/// regression that motivated the recent `transition_to_trailers` fallback in
/// [`send`][super::super::send]: previously, by the time the cursor reached `Body` EOF, the
/// only pickup site for `pending_trailers` had already run, and the trailers were lost.
#[test]
fn submit_trailers_lands_on_wire_after_body_parked() {
    use crate::{h2::frame::Frame, headers::hpack::PseudoHeaders};

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // With no prelude body, submit_upgrade frames HEADERS, then on the first Body tick
    // signals submission completion and lazily swaps in an `H2OutboundReader` as the
    // continuation source, leaving the cursor parked in Body until either bytes appear in
    // the outbound queue or `outbound_close_requested` flips.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx
        .connection
        .submit_upgrade(1, pseudos, Headers::new(), None);

    // Tick: HEADERS go out, cursor parks in Body (empty outbound, close not requested).
    let _ = fx.tick();
    let headers_round = fx.next_outbound_frames();
    assert!(
        headers_round.iter().any(|f| matches!(
            f,
            Frame::Headers {
                stream_id: 1,
                end_stream: false,
                ..
            }
        )),
        "response HEADERS (without END_STREAM) should be on the wire after first tick; got \
         {headers_round:?}",
    );

    // Outside the driver task: stage trailers + request close. The driver's send pump
    // must pick this up on its next tick despite the cursor being parked.
    let mut trailers = Headers::new();
    trailers.insert("grpc-status", "0");
    fx.connection
        .submit_trailers(1, trailers)
        .expect("submit_trailers on a live stream");

    let _ = fx.tick();
    let trailing = fx.next_outbound_frames();
    let trailing_headers = trailing
        .iter()
        .filter(|f| {
            matches!(
                f,
                Frame::Headers {
                    stream_id: 1,
                    end_stream: true,
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        trailing_headers, 1,
        "exactly one trailing HEADERS with END_STREAM should land on the wire after \
         submit_trailers; got {trailing:?}",
    );
}

/// An extended-CONNECT upgrade stream sitting at `UpgradeOpen` with an empty outbound
/// queue (handler hasn't written, peer hasn't sent more) must let the driver park —
/// returning `Poll::Pending` *without* self-waking. The `SendCursor` is parked in `Body`
/// because the upgrade body's `poll_read` returned `Pending` (it registered the outbound
/// waker), so there's no progress to make until an external wake arrives.
///
/// Regression: `has_pending_outbound_progress` used to report `true` for any `Body`-phase
/// cursor with a positive send window, ignoring that the body had parked. That defeated
/// `park`, so the driver burned through `copy_loops_per_yield` every poll and re-armed via
/// the cooperative-yield `wake_by_ref` — a busy-spin emitting hundreds of thousands of
/// `drive` log lines instead of sleeping. Asserting the waker isn't fired pins the park.
#[test]
fn idle_upgrade_open_stream_parks_without_self_waking() {
    use crate::headers::hpack::PseudoHeaders;

    /// Wake counter so we can tell a clean park (no wake) from a self-wake spin.
    struct CountingWaker(std::sync::atomic::AtomicUsize);
    impl Wake for CountingWaker {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // Drive into the parked-upgrade state: HEADERS go out, the cursor parks in Body with an
    // empty outbound queue and no close requested.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx
        .connection
        .submit_upgrade(1, pseudos, Headers::new(), None);
    let _ = fx.tick();
    let _ = fx.next_outbound_bytes();

    // The next poll has no work: no inbound frame, no outbound bytes, body parked. The
    // driver must register on its wakers and return Pending without re-arming itself.
    let counter = Arc::new(CountingWaker(std::sync::atomic::AtomicUsize::new(0)));
    let waker = Waker::from(counter.clone());
    let mut cx = Context::from_waker(&waker);
    let polled = fx.driver.drive(&mut cx);
    assert!(
        matches!(polled, Poll::Pending),
        "idle upgrade-open driver should park, got {polled:?}",
    );
    assert_eq!(
        counter.0.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "driver self-woke instead of parking — busy-spin on an idle bidi/upgrade tunnel",
    );
}

/// A server that finishes responding (trailing HEADERS + END_STREAM) while the peer's
/// request half is still open is only at half-closed (local), not closed (RFC 9113
/// §5.1). The peer's subsequent END_STREAM — a zero-length DATA frame closing its
/// request half — is legal and must complete the stream cleanly. The bug this pins:
/// server-role teardown removes the stream on send completion regardless of recv state,
/// so the peer's END_STREAM lands on a stream the driver has already forgotten and is
/// answered with a spurious `RST_STREAM(STREAM_CLOSED)`. That RST races back to the peer
/// and destroys the just-delivered trailers — the gRPC "stream ended without grpc-status
/// trailer" failure under load.
#[test]
fn peer_end_stream_after_server_trailers_is_not_reset() {
    use crate::headers::hpack::PseudoHeaders;

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Open the request stream WITHOUT END_STREAM — the peer's request half stays open,
    // exactly as a gRPC client's upgrade-style request stream does before it has sent its
    // own terminator.
    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // Server responds via the upgrade path and stages trailers, completing its send half
    // while the peer's request half is still open.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx
        .connection
        .submit_upgrade(1, pseudos, Headers::new(), None);
    let _ = fx.tick();
    let _ = fx.next_outbound_frames();

    let mut trailers = Headers::new();
    trailers.insert("grpc-status", "0");
    fx.connection
        .submit_trailers(1, trailers)
        .expect("submit_trailers on a live stream");
    let _ = fx.tick();
    let trailing = fx.next_outbound_frames();
    assert!(
        trailing.iter().any(|f| matches!(
            f,
            Frame::Headers {
                stream_id: 1,
                end_stream: true,
                ..
            }
        )),
        "server's trailing HEADERS with END_STREAM should be on the wire; got {trailing:?}",
    );

    // Now the peer closes its request half: a zero-length DATA frame with END_STREAM.
    // This arrives strictly after the server's trailers — the deterministic version of
    // the load-dependent race.
    fx.peer_data(1, &[], true);
    let _ = fx.tick();

    let after = fx.next_outbound_frames();
    assert!(
        !after
            .iter()
            .any(|f| matches!(f, Frame::RstStream { stream_id: 1, .. })),
        "peer's END_STREAM on a half-closed-local stream must close cleanly, not earn a \
         RST_STREAM; got {after:?}",
    );
}

/// A bidirectional upgrade stream (server still streaming responses) must survive the
/// peer half-closing its *request* side. RFC 9113 §5.1: the peer's `END_STREAM` only moves
/// the stream to half-closed (remote); the server's send half stays open and the handler
/// keeps writing. The bug this pins: the server-role both-done teardown treated the upgrade
/// stream's submit-resolved signal — fired when `SubmitSend` resolved so the conn task could
/// dispatch `Handler::upgrade` — as "send half finished." Combined with the peer's
/// `END_STREAM` (recv done) it tore the stream down mid-bidi; the handler's subsequent writes
/// then vanished (queued on a stream the driver had forgotten) and trailers failed with "not
/// connected" — the gRPC bidi-streaming regression. The fix routes teardown through the
/// lifecycle's `LocalClosed` state, which an open `UpgradeOpen` stream never reaches until
/// its own terminator is framed.
#[test]
fn peer_half_close_does_not_tear_down_open_upgrade() {
    use crate::headers::hpack::PseudoHeaders;

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    // Bidi request stream: open WITHOUT END_STREAM so the recv side stays live.
    fx.peer_open_stream(1, Method::Post, "/", false);
    let mut conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // Server upgrades (no prelude). HEADERS go out without END_STREAM; the cursor resolves
    // the SubmitSend future and parks in Body on the empty outbound queue — UpgradeOpen,
    // handler now in control of the bidi stream.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx
        .connection
        .submit_upgrade(1, pseudos, Headers::new(), None);
    let _ = fx.tick();
    let _ = fx.next_outbound_frames();

    // Peer half-closes its request side mid-stream — routine for a bidi client that's done
    // sending while the server keeps responding. Zero-length DATA with END_STREAM.
    fx.peer_data(1, &[], true);
    let _ = fx.tick();

    assert!(
        fx.connection.streams_lock().contains_key(&1),
        "peer half-close on an open upgrade stream tore the whole stream down; the server's send \
         half is still live and the handler is still writing",
    );

    // Functional confirmation: the handler writes a response chunk, which the send pump
    // must frame as DATA. On the bug path the stream is gone, so these bytes queue on an
    // orphaned StreamState and never reach the wire.
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let _ = Pin::new(&mut conn.transport).poll_write(&mut cx, b"hello bidi");
    let _ = fx.tick();
    let frames = fx.next_outbound_frames();
    assert!(
        frames
            .iter()
            .any(|f| matches!(f, Frame::Data { stream_id: 1, .. })),
        "handler's post-half-close write should be framed as DATA on stream 1; got {frames:?}",
    );
}

/// The completing half of the recv-first ordering: peer half-closes its request side while
/// the upgrade is open, *then* the handler finishes (final write + trailers). The stream
/// must terminate cleanly — trailing HEADERS(END_STREAM) on the wire — and only then be
/// removed. The companion [`peer_half_close_does_not_tear_down_open_upgrade`] pins that the
/// half-close alone doesn't tear down; this pins that completion afterward still works and
/// the both-done removal fires in the recv-then-send order (the order the old
/// `send.completed`-as-send-done conflation got wrong).
#[test]
fn upgrade_completes_after_peer_half_closes_first() {
    use crate::headers::hpack::PseudoHeaders;

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Post, "/", false);
    let mut conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx
        .connection
        .submit_upgrade(1, pseudos, Headers::new(), None);
    let _ = fx.tick();
    let _ = fx.next_outbound_frames();

    // Recv side closes first, mid-upgrade.
    fx.peer_data(1, &[], true);
    let _ = fx.tick();
    assert!(
        fx.connection.streams_lock().contains_key(&1),
        "stream must survive the peer's half-close while the handler is still open",
    );

    // Handler finishes: a final write, then trailers (which request close).
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let _ = Pin::new(&mut conn.transport).poll_write(&mut cx, b"final");
    let _ = fx.tick();
    let mut trailers = Headers::new();
    trailers.insert("grpc-status", "0");
    fx.connection
        .submit_trailers(1, trailers)
        .expect("submit_trailers on a live stream");

    // Drain to completion, collecting every frame the closing sequence emits.
    let mut frames = Vec::new();
    for _ in 0..4 {
        let _ = fx.tick();
        frames.extend(fx.next_outbound_frames());
    }
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::Headers {
                stream_id: 1,
                end_stream: true,
                ..
            }
        )),
        "handler completion should emit trailing HEADERS(END_STREAM); got {frames:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "with the send terminator framed and the peer already half-closed, the server should \
         remove the stream",
    );
}

/// A peer `RST_STREAM` on an open upgrade stream (gRPC client cancelling an in-flight RPC)
/// must terminate the stream *and* stop accepting handler writes — otherwise the handler's
/// subsequent `poll_write`s queue onto a `StreamState` the driver has dropped from its map
/// and silently vanish. The stream moves to the terminal `Reset` state, so writes return
/// `BrokenPipe` and the application learns the peer is gone.
#[test]
fn peer_rst_during_open_upgrade_rejects_further_writes() {
    use crate::headers::hpack::PseudoHeaders;

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Post, "/", false);
    let mut conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx
        .connection
        .submit_upgrade(1, pseudos, Headers::new(), None);
    let _ = fx.tick();
    let _ = fx.next_outbound_frames();

    // Peer cancels the RPC mid-stream.
    fx.peer_rst_stream(1, H2ErrorCode::Cancel);
    let _ = fx.tick();
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "peer RST_STREAM should remove the stream from the map",
    );

    // A handler write after the reset must fail loudly, not disappear into an orphan buffer.
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    match Pin::new(&mut conn.transport).poll_write(&mut cx, b"after reset") {
        Poll::Ready(Err(e)) => assert_eq!(
            e.kind(),
            std::io::ErrorKind::BrokenPipe,
            "post-RST write should report BrokenPipe",
        ),
        other => panic!("post-RST write should fail with BrokenPipe, got {other:?}"),
    }
}

/// A DATA frame arriving after the peer has already sent its own `END_STREAM` (recv half
/// half-closed-remote) is a `STREAM_CLOSED` stream error (RFC 9113 §5.1). This is the dual
/// of [`peer_end_stream_after_server_trailers_is_not_reset`]: a *legal* frame after our
/// half-closed-local must not be reset, but an *illegal* DATA after the peer's own
/// `END_STREAM` must be. Easy to conflate when refactoring the recv path.
#[test]
fn peer_data_after_its_own_end_stream_is_reset() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    fx.peer_data(1, b"hello", false);
    let _ = fx.tick();
    fx.peer_data(1, &[], true); // legal END_STREAM closing the request half
    let _ = fx.tick();
    let _ = fx.next_outbound_frames();

    // Illegal: more DATA after the peer's own END_STREAM.
    fx.peer_data(1, b"extra", false);
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
        "DATA after the peer's own END_STREAM must earn RST_STREAM(STREAM_CLOSED); got {frames:?}",
    );
}

/// An inbound peer `GOAWAY` mid-upgrade triggers graceful close but must not abruptly tear
/// down an in-flight bidi stream — the handler keeps framing until it completes. This is
/// the GOAWAY-analog of [`peer_half_close_does_not_tear_down_open_upgrade`]; the risk it
/// pins is GOAWAY routing through `begin_close` racing the handler's remaining writes.
#[test]
fn inbound_goaway_does_not_tear_down_open_upgrade() {
    use crate::headers::hpack::PseudoHeaders;
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Post, "/", false);
    let mut conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx
        .connection
        .submit_upgrade(1, pseudos, Headers::new(), None);
    let _ = fx.tick();
    let _ = fx.next_outbound_frames();

    // Peer announces graceful shutdown while our upgrade stream is live.
    fx.peer_goaway(0, H2ErrorCode::NoError);
    let _ = fx.tick();
    assert!(
        fx.connection.streams_lock().contains_key(&1),
        "inbound GOAWAY tore down an in-flight upgrade stream mid-bidi",
    );

    // The handler's continued write must still frame as DATA — the send pump runs in
    // Closing, so a stream mid-upgrade keeps making progress after the GOAWAY.
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let _ = Pin::new(&mut conn.transport).poll_write(&mut cx, b"post-goaway");
    let _ = fx.tick();
    let frames = fx.next_outbound_frames();
    assert!(
        frames
            .iter()
            .any(|f| matches!(f, Frame::Data { stream_id: 1, .. })),
        "handler's post-GOAWAY write should be framed as DATA on stream 1; got {frames:?}",
    );
}

/// A peer `RST_STREAM` on a normal (non-upgrade) in-flight request — arriving before the
/// handler has submitted its response — removes the stream, and a *later* `submit_send`
/// from the still-running handler must resolve cleanly as `NotConnected` rather than
/// panicking or hanging on a stream the driver has already dropped from its map.
#[test]
fn peer_rst_on_in_flight_request_errors_later_submit() {
    use crate::{Body, headers::hpack::PseudoHeaders};
    use std::{future::Future, io::ErrorKind};

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    fx.peer_rst_stream(1, H2ErrorCode::Cancel);
    let _ = fx.tick();
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "peer RST_STREAM should remove the in-flight stream from the map",
    );

    // The handler, unaware of the RST, now submits its response.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let mut submit = fx.connection.submit_send(
        1,
        pseudos,
        Headers::new(),
        Some(Body::new_static(b"hi" as &[u8])),
    );
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    match Pin::new(&mut submit).poll(&mut cx) {
        Poll::Ready(Err(e)) => assert_eq!(
            e.kind(),
            ErrorKind::NotConnected,
            "submit_send on a reset/removed stream should resolve NotConnected",
        ),
        other => panic!("submit_send after RST should resolve NotConnected, got {other:?}"),
    }
}

/// Coalesced close: the peer's `END_STREAM` and the handler's terminating response land in
/// a single `drive` budget, not across separate ticks like the other close-ordering tests.
/// Pins that the server-role both-done teardown fires regardless of which half the driver
/// observes closing first within one tick — the send pump (step 2) runs before the read
/// pump (step 6), so the response's `END_STREAM` is framed first and the peer's is read
/// second, both in the same `drive`.
#[test]
fn same_tick_peer_end_stream_and_response_terminator_closes_cleanly() {
    use crate::{Body, headers::hpack::PseudoHeaders};

    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // Stage BOTH the peer's END_STREAM and the response before a single tick.
    fx.peer_data(1, &[], true);
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_send(
        1,
        pseudos,
        Headers::new(),
        Some(Body::new_static(b"hi" as &[u8])),
    );

    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::Data {
                stream_id: 1,
                end_stream: true,
                ..
            }
        )),
        "response should terminate with DATA(END_STREAM); got {frames:?}",
    );
    assert!(
        !frames
            .iter()
            .any(|f| matches!(f, Frame::RstStream { stream_id: 1, .. })),
        "coalesced both-halves close must not emit a spurious RST_STREAM; got {frames:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "with both halves closed in one tick, the server should remove the stream",
    );
}

/// A trailing HEADERS block missing END_STREAM is a §8.1 violation → stream-level
/// `RST_STREAM(PROTOCOL_ERROR)`. (Trailers MUST terminate the stream.)
#[test]
fn trailing_headers_without_end_stream_is_reset() {
    use crate::headers::hpack::PseudoHeaders;
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };
    let _ = fx.next_outbound_frames();

    // Second HEADERS on the open stream = trailers, but without END_STREAM.
    let mut trailers = Headers::new();
    trailers.insert("grpc-status", "0");
    fx.peer_headers(1, PseudoHeaders::default(), &trailers, false);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::RstStream {
                stream_id: 1,
                error_code: H2ErrorCode::ProtocolError,
            }
        )),
        "trailing HEADERS without END_STREAM must earn RST_STREAM(PROTOCOL_ERROR); got {frames:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "the malformed-trailer stream should be removed",
    );
}

/// A trailing HEADERS block carrying a pseudo-header is a §8.1 violation → stream-level
/// `RST_STREAM(PROTOCOL_ERROR)`. (Trailers MUST NOT contain pseudo-headers.)
#[test]
fn trailing_headers_with_pseudo_header_is_reset() {
    use crate::headers::hpack::PseudoHeaders;
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();

    fx.peer_open_stream(1, Method::Post, "/", false);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };
    let _ = fx.next_outbound_frames();

    // Trailers carrying a pseudo-header (`:status`), with END_STREAM set.
    fx.peer_headers(
        1,
        PseudoHeaders::default().with_status(Status::Ok),
        &Headers::new(),
        true,
    );
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::RstStream {
                stream_id: 1,
                error_code: H2ErrorCode::ProtocolError,
            }
        )),
        "trailing HEADERS with a pseudo-header must earn RST_STREAM(PROTOCOL_ERROR); got \
         {frames:?}",
    );
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "the malformed-trailer stream should be removed",
    );
}

/// Peer `RST_STREAM` while the upgrade send cursor is still mid-*prelude* (before the lazy
/// swap to the outbound-queue continuation) — a distinct `Body` sub-state from the
/// parked-on-reader case in [`peer_rst_during_open_upgrade_rejects_further_writes`]. The
/// teardown must still reset cleanly: stream removed, post-RST writes rejected. A tiny send
/// window keeps the prelude from draining, pinning the cursor in the prelude sub-state.
#[test]
fn peer_rst_during_prelude_body_phase_rejects_writes() {
    use crate::{Body, h2::settings::H2Settings, headers::hpack::PseudoHeaders};

    let mut fx = DriverFixture::new_server();
    // 2-byte send window: the prelude can't fully drain, so the cursor parks mid-prelude.
    fx.complete_handshake_with_peer_settings(H2Settings::default().with_initial_window_size(2));

    fx.peer_open_stream(1, Method::Post, "/", false);
    let mut conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };

    // Upgrade with a prelude larger than the window: response HEADERS + a partial prelude
    // DATA go out, then the cursor parks mid-prelude (continuation not yet started).
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_upgrade(
        1,
        pseudos,
        Headers::new(),
        Some(Body::new_static(b"prelude-bytes" as &[u8])),
    );
    let _ = fx.tick();
    let _ = fx.next_outbound_frames();

    // Peer cancels while the prelude is still draining.
    fx.peer_rst_stream(1, H2ErrorCode::Cancel);
    let _ = fx.tick();
    assert!(
        !fx.connection.streams_lock().contains_key(&1),
        "peer RST mid-prelude should remove the stream",
    );

    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    match Pin::new(&mut conn.transport).poll_write(&mut cx, b"after reset") {
        Poll::Ready(Err(e)) => assert_eq!(
            e.kind(),
            std::io::ErrorKind::BrokenPipe,
            "post-RST write should report BrokenPipe",
        ),
        other => panic!("post-RST write should fail with BrokenPipe, got {other:?}"),
    }
}
