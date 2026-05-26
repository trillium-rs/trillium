//! Client-role wire tests: the driver runs the *client* side and the test plays the
//! *server* peer. This quadrant was previously exercised only on the happy path via the
//! integration `upgrade_matrix.rs`; here we drive it adversarially at the frame level
//! (server RST / GOAWAY / illegal frames against an in-flight client stream).
//!
//! Setup differs from the server-role tests: streams are opened locally via
//! [`H2Connection::open_stream`] rather than by an inbound HEADERS, and responses arrive as
//! peer HEADERS on the client's own (odd) stream ids.

use super::fixture::*;
use crate::{
    Body, Buffer, Headers, KnownHeaderName, Method, ProtocolSession, ReceivedBody,
    ReceivedBodyState, Status,
    h2::{H2ErrorCode, H2Transport, SubmitSend, acceptor::types::DriverState, frame::Frame},
    headers::hpack::PseudoHeaders,
};
use futures_lite::io::AsyncRead;
use std::{
    future::Future,
    net::Shutdown,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

/// A waker that counts how many times it (or a clone) was woken. Lets a test assert that an
/// event which arrives *after* the driver parked actually re-wakes the driver task — the
/// thing a synchronous re-`tick()` would paper over.
struct CountingWaker(AtomicUsize);
impl Wake for CountingWaker {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Open a body-less GET on a freshly-handshaked client fixture, returning the stream id and
/// the held `SubmitSend` + `H2Transport` (kept alive by the caller so the stream isn't
/// torn down by a dropped transport). Asserts the request HEADERS(END_STREAM) is framed.
fn open_get(fx: &mut DriverFixture) -> (u32, SubmitSend, H2Transport) {
    let pseudos = PseudoHeaders::default()
        .with_method(Method::Get)
        .with_path("/")
        .with_scheme("http")
        .with_authority("test");
    let (id, submit, transport) = fx
        .connection
        .open_stream(pseudos, Headers::new(), None)
        .expect("open_stream on a running client connection");
    let _ = fx.tick();
    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::Headers {
                stream_id,
                end_stream: true,
                ..
            } if *stream_id == id
        )),
        "body-less request should frame HEADERS(END_STREAM) on stream {id}; got {frames:?}",
    );
    (id, submit, transport)
}

/// Fixture sanity check: a client opens a stream, the request HEADERS go out, the server
/// peer responds with HEADERS, and the client surfaces the response via `response_headers`.
/// Validates the client-role harness before the adversarial tests rely on it.
#[test]
fn client_request_response_round_trip() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();

    let (id, _submit, _transport) = open_get(&mut fx);
    assert_eq!(id, 1, "first client-allocated stream id is 1");

    fx.peer_response_headers(id, Status::Ok, true);
    let _ = fx.tick();

    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut resp = fx.connection.response_headers(id);
    match Pin::new(&mut resp).poll(&mut cx) {
        Poll::Ready(Ok(_fields)) => {}
        other => {
            panic!("response_headers should resolve Ok after the peer's HEADERS; got {other:?}")
        }
    }
}

/// A server `RST_STREAM` on an in-flight client stream (response not yet received) must
/// surface to the waiting client as a clean `response_headers` error rather than hanging,
/// and remove the stream. This is the client-side dual of the server-role
/// `peer_rst_during_open_upgrade_rejects_further_writes`.
#[test]
fn server_rst_on_in_flight_client_stream_surfaces_to_response_waiter() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();

    let (id, _submit, _transport) = open_get(&mut fx);

    // Server cancels before sending any response HEADERS.
    fx.peer_rst_stream(id, H2ErrorCode::Cancel);
    let _ = fx.tick();
    assert!(
        !fx.connection.streams_lock().contains_key(&id),
        "server RST should remove the client stream from the map",
    );

    // A client awaiting the response must get a terminal error, not park forever.
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut resp = fx.connection.response_headers(id);
    match Pin::new(&mut resp).poll(&mut cx) {
        Poll::Ready(Err(_)) => {}
        other => panic!("response_headers on a server-reset stream should error, got {other:?}"),
    }
}

/// A server `GOAWAY` followed by connection close, while a client request is still awaiting
/// its response, must resolve the parked `response_headers` waiter with an error — not hang.
/// This asserts `ResponseHeaders`' own documented contract ("ConnectionAborted — recv side
/// reached eof ... peer sent GOAWAY ... or otherwise tore the connection down").
#[test]
fn server_goaway_resolves_pending_response_waiter() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, _submit, _transport) = open_get(&mut fx);

    // Clone the Arc so the borrow the future holds is independent of `fx` (lets us keep the
    // future parked across `fx.tick()` calls — mirrors a real request task awaiting while
    // the driver task runs).
    let conn = fx.connection.clone();
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut resp = conn.response_headers(id);

    assert!(
        matches!(Pin::new(&mut resp).poll(&mut cx), Poll::Pending),
        "no response yet — the waiter should park",
    );

    // Server announces shutdown and closes the connection without ever responding.
    fx.peer_goaway(0, H2ErrorCode::NoError);
    fx.peer.shutdown(Shutdown::Both);
    for _ in 0..4 {
        let _ = fx.tick();
    }

    match Pin::new(&mut resp).poll(&mut cx) {
        Poll::Ready(Err(_)) => {}
        other => panic!(
            "response_headers must resolve with an error once the connection dies after GOAWAY \
             (per its documented ConnectionAborted contract); got {other:?}",
        ),
    }
}

/// Regression probe
///
/// The deadlock's hung side is the *client* (GOAWAY `last_stream_id=0`): it enters `Closing`
/// by mirroring the server's GOAWAY while a request stream is still recv-open (awaiting a
/// response that will never come), then must consume the server's `RST_STREAM` to clear its
/// `has_pending_recv` drain gate. The pass/hang traces are identical up to entering `Closing`;
/// in the hang the client parks without ever consuming that RST.
///
/// This buffers GOAWAY *and* the RST together, then ticks: if the client processes only the
/// GOAWAY (→ Closing) but fails to drain the already-buffered RST, it stays stuck — exactly
/// the hang. We hold the request transport so the stream isn't torn down by a local drop (the
/// gate must clear from the inbound RST, not from our own teardown).
#[test]
fn client_drains_buffered_rst_after_mirroring_peer_goaway() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, _submit, _transport) = open_get(&mut fx);

    // Both frames land in the client's read buffer at once: a graceful GOAWAY, then the
    // server's RST for the in-flight stream.
    fx.peer_goaway(1, H2ErrorCode::NoError);
    fx.peer_rst_stream(id, H2ErrorCode::Cancel);
    for _ in 0..4 {
        let _ = fx.tick();
    }

    assert!(
        !fx.connection.streams_lock().contains_key(&id),
        "client must consume the buffered RST_STREAM and remove the stream; got it still present",
    );
    assert_eq!(
        fx.driver.state,
        DriverState::Drained,
        "after mirroring GOAWAY and consuming the RST, the client's drain gate should clear",
    );
}

/// Regression probe
///
/// The hung side is the client driver task: it mirrors the server's GOAWAY into `Closing`
/// with a stream still recv-open, parks, and must be re-woken when the server's `RST_STREAM`
/// arrives so it can drain the gate. Unlike
/// [`client_drains_buffered_rst_after_mirroring_peer_goaway`] (which buffers both frames and
/// re-`tick()`s synchronously — masking any lost wake), this separates them in time and drives with
/// a [`CountingWaker`]:
///
/// 1. deliver only the GOAWAY; drive until the driver returns `Pending` (parked in `Closing`);
/// 2. snapshot the wake count;
/// 3. deliver the RST; a correctly-registered read waker fires (the `TestTransport` write wakes
///    whatever waker the last `poll_read` registered).
///
/// If the wake count does *not* advance, the parked driver never learns the RST arrived — a
/// lost-wake deadlock, exactly the production hang. If it *does* advance, the fixture can't
/// reproduce it and the real cause is something `TestTransport` doesn't model (socket write
/// backpressure / a cross-thread wake race).
#[test]
fn client_parked_in_closing_is_rewoken_by_late_rst() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, _submit, _transport) = open_get(&mut fx);

    let counter = Arc::new(CountingWaker(AtomicUsize::new(0)));
    let waker = Waker::from(counter.clone());
    let mut cx = Context::from_waker(&waker);

    // Server announces graceful shutdown — but no RST yet. The client mirrors it into Closing
    // and, with the stream still recv-open, parks awaiting more inbound.
    fx.peer_goaway(1, H2ErrorCode::NoError);
    let mut polls = 0;
    loop {
        match fx.driver.drive(&mut cx) {
            Poll::Pending => break,
            Poll::Ready(Some(_)) => {}
            Poll::Ready(None) => panic!("driver finished before the in-flight stream drained"),
        }
        polls += 1;
        assert!(polls < 100, "driver never settled to Pending");
    }
    assert_eq!(
        fx.driver.state,
        DriverState::Closing,
        "client should be Closing after mirroring the peer GOAWAY",
    );
    assert!(
        fx.connection.streams_lock().contains_key(&id),
        "the in-flight recv-open stream should still be holding the drain gate",
    );

    let wakes_before = counter.0.load(Ordering::SeqCst);

    // The server's RST for the in-flight stream lands *after* the driver parked.
    fx.peer_rst_stream(id, H2ErrorCode::Cancel);

    let wakes_after = counter.0.load(Ordering::SeqCst);
    assert!(
        wakes_after > wakes_before,
        "an RST arriving after the driver parked in Closing must re-wake the driver task (was \
         {wakes_before}, now {wakes_after}); no wake means the lost-wake deadlock",
    );
}

/// Regression: when the client has already received the full response — including trailers —
/// and the server then sends `RST_STREAM(NoError)` (because the client's request/send half was
/// still open), the already-received trailers must still be surfaced. The RST path used to
/// remove the stream from the map outright, so the body's EOF `take_trailers` found nothing and
/// the application saw a clean EOF with no trailers.
#[test]
fn server_rst_after_trailers_preserves_trailers_while_send_half_open() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();

    // A request whose body is larger than the peer's default 65535-byte initial window: after
    // HEADERS + one full window of DATA the send half is still Open (the body can't finish
    // until the peer grants more window, which it never does).
    let pseudos = PseudoHeaders::default()
        .with_method(Method::Post)
        .with_path("/")
        .with_scheme("http")
        .with_authority("test");
    let (id, _submit, _transport) = fx
        .connection
        .open_stream(
            pseudos,
            Headers::new(),
            Some(Body::new_static(vec![0u8; 70_000])),
        )
        .expect("open_stream on a running client connection");
    let _ = fx.tick();
    let _ = fx.next_outbound_frames();

    // Full response: HEADERS (no END_STREAM) then a trailing HEADERS (END_STREAM).
    fx.peer_response_headers(id, Status::Ok, false);
    let _ = fx.tick();
    let mut trailers = Headers::new();
    trailers.insert("grpc-status", "0");
    fx.peer_trailers(id, &trailers);
    let _ = fx.tick();

    assert!(
        fx.connection.streams_lock().contains_key(&id),
        "stream should still be tracked after receiving trailers (send half still open)",
    );

    // The server resets our still-open send half with NoError — it has what it needs and
    // doesn't want the rest of the request body.
    fx.peer_rst_stream(id, H2ErrorCode::NoError);
    let _ = fx.tick();

    // The trailers received before the RST must still be retrievable — this is exactly what
    // `ReceivedBody` calls at EOF to populate `conn.response_trailers`.
    let recovered = fx.connection.take_trailers(id);
    assert!(
        recovered.is_some_and(|t| t.get_str("grpc-status") == Some("0")),
        "trailers received before RST_STREAM(NoError) must be preserved, not discarded",
    );
}

/// A server DATA frame after its own response `END_STREAM` is illegal — the client must
/// answer with `RST_STREAM(STREAM_CLOSED)`. Client-side dual of the server-role
/// `peer_data_after_its_own_end_stream_is_reset`.
#[test]
fn server_data_after_its_own_end_stream_is_reset() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, _submit, _transport) = open_get(&mut fx);

    // Complete response: HEADERS with END_STREAM, no body.
    fx.peer_response_headers(id, Status::Ok, true);
    let _ = fx.tick();
    let _ = fx.next_outbound_frames();

    // Illegal: more DATA after the server's own END_STREAM.
    fx.peer_data(id, b"extra", false);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::RstStream {
                stream_id,
                error_code: H2ErrorCode::StreamClosed,
            } if *stream_id == id
        )),
        "client must RST_STREAM(STREAM_CLOSED) on DATA after the server's END_STREAM; got \
         {frames:?}",
    );
}

/// An interim (1xx) response HEADERS frame must be discarded, not surfaced as the response:
/// the client waits for the final HEADERS. RFC 9110 §15.2 — informational responses precede
/// the final and their headers must not be merged into it. The h2 path
/// (`finalize_response_headers`) is distinct from the h1 interim handling tested in
/// `client/tests/{one_hundred_continue,early_hints}.rs`, so it gets its own coverage here.
#[test]
fn client_discards_interim_response_and_surfaces_final() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, _submit, _transport) = open_get(&mut fx);

    // Interim 100 Continue (no END_STREAM) — discarded.
    fx.peer_response_headers(id, Status::Continue, false);
    let _ = fx.tick();
    // The final response.
    fx.peer_response_headers(id, Status::Ok, true);
    let _ = fx.tick();

    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut resp = fx.connection.response_headers(id);
    match Pin::new(&mut resp).poll(&mut cx) {
        Poll::Ready(Ok(fields)) => assert_eq!(
            fields.pseudo_headers().status(),
            Some(Status::Ok),
            "the surfaced response must be the final 200, not the discarded interim 1xx",
        ),
        other => panic!("expected the final response headers to surface, got {other:?}"),
    }
}

/// Response-side §8.1.2.6: a response body longer than its declared `content-length` is a
/// stream-level `PROTOCOL_ERROR`. The client must `RST_STREAM(PROTOCOL_ERROR)` rather than
/// silently truncate the body at the declared length. Server-role dual:
/// `data_exceeding_content_length_is_protocol_error`.
#[test]
fn server_response_body_exceeding_content_length_is_reset() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, _submit, _transport) = open_get(&mut fx);

    // Response declares content-length: 1 but the body is 4 bytes.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let mut fields = Headers::new();
    fields.insert(KnownHeaderName::ContentLength, "1");
    fx.peer_headers(id, pseudos, &fields, false);
    let _ = fx.tick();
    let _ = fx.next_outbound_frames();

    fx.peer_data(id, b"test", true);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::RstStream { stream_id, error_code: H2ErrorCode::ProtocolError }
                if *stream_id == id
        )),
        "a response body past content-length must earn RST_STREAM(PROTOCOL_ERROR); got {frames:?}",
    );
    assert!(!fx.connection.streams_lock().contains_key(&id));
}

/// Control for the response-side §8.1.2.6 check: a response body matching its declared
/// `content-length` is well-formed — no reset, stream survives to deliver the body.
#[test]
fn server_response_body_matching_content_length_is_accepted() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, _submit, _transport) = open_get(&mut fx);

    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let mut fields = Headers::new();
    fields.insert(KnownHeaderName::ContentLength, "4");
    fx.peer_headers(id, pseudos, &fields, false);
    let _ = fx.tick();
    let _ = fx.next_outbound_frames();

    fx.peer_data(id, b"test", true);
    let _ = fx.tick();

    let frames = fx.next_outbound_frames();
    assert!(
        !frames.iter().any(|f| matches!(
            f,
            Frame::RstStream { stream_id, error_code: H2ErrorCode::ProtocolError }
                if *stream_id == id
        )),
        "a response body matching content-length must not be reset; got {frames:?}",
    );
}

/// A spec-forbidden `END_STREAM` on an interim (1xx) HEADERS frame must abort the response
/// waiter rather than hang it: the driver honors the `END_STREAM` (recv half closes) so an
/// awaiting `response_headers` resolves with a connection-aborted error instead of blocking
/// forever on a final response that the (now closed) stream can never deliver. This
/// hang-prevention edge has no h1 analog.
#[test]
fn client_interim_response_with_end_stream_aborts_waiter() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, _submit, _transport) = open_get(&mut fx);

    fx.peer_response_headers(id, Status::Continue, true);
    let _ = fx.tick();

    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut resp = fx.connection.response_headers(id);
    match Pin::new(&mut resp).poll(&mut cx) {
        Poll::Ready(Err(_)) => {}
        other => panic!(
            "an interim response with END_STREAM should abort the response waiter, not hang or \
             surface a response; got {other:?}",
        ),
    }
}

/// Regression: a response carrying `content-length` must not let the body declare EOF (and
/// harvest trailers) until the stream's `END_STREAM` arrives. In HTTP/2 the trailing HEADERS
/// follow the DATA, so a body that ends the instant `content-length` is satisfied races the
/// driver's trailer stash and loses — surfacing an empty trailer set. gRPC unary/client-stream
/// responses set `content-length` (connect-go buffers them), which made the rotating conformance
/// "empty trailers → Unknown" flake. `content-length: 0` (an empty/no-message response) is the
/// deterministic case: the body would otherwise complete on the very first poll, before any
/// trailer could possibly have arrived.
#[test]
fn content_length_response_defers_eof_until_end_stream_so_trailers_survive() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, _submit, mut transport) = open_get(&mut fx);

    // Response head: 200 with `content-length: 0`, NO END_STREAM — trailers still to come.
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let mut fields = Headers::new();
    fields.insert(KnownHeaderName::ContentLength, "0");
    fx.peer_headers(id, pseudos, &fields, false);
    let _ = fx.tick();

    // Read the response body exactly as trillium-client does: `content_length` from the head,
    // an h2 protocol session so the End transition harvests trailers via `take_trailers`.
    let mut buffer = Buffer::with_capacity(64);
    let mut state = ReceivedBodyState::new_h2();
    let mut received_trailers: Option<Headers> = None;
    let mut body: ReceivedBody<'_, H2Transport> = ReceivedBody::new(
        Some(0),
        &mut buffer,
        &mut transport,
        &mut state,
        None,
        encoding_rs::UTF_8,
    )
    .with_protocol_session(ProtocolSession::Http2 {
        connection: fx.connection.clone(),
        stream_id: id,
    })
    .with_trailers(&mut received_trailers);

    let waker = Waker::from(Arc::new(CountingWaker(AtomicUsize::new(0))));
    let mut cx = Context::from_waker(&waker);
    let mut buf = [0u8; 16];

    // Before END_STREAM the body must park, not report EOF. With the content-length-terminal
    // bug this returns `Ready(Ok(0))` on the first poll and harvests no trailers.
    assert!(
        Pin::new(&mut body).poll_read(&mut cx, &mut buf).is_pending(),
        "body must not declare EOF on content-length alone before END_STREAM (would lose trailers)",
    );

    // Trailing HEADERS (END_STREAM) arrive carrying grpc-status.
    let mut trailers = Headers::new();
    trailers.insert("grpc-status", "0");
    fx.peer_trailers(id, &trailers);
    let _ = fx.tick();

    // Now the body reaches EOF and surfaces the trailers it had to wait for.
    assert!(
        matches!(
            Pin::new(&mut body).poll_read(&mut cx, &mut buf),
            Poll::Ready(Ok(0))
        ),
        "body should reach clean EOF once END_STREAM has arrived",
    );
    assert_eq!(
        received_trailers.as_ref().and_then(|t| t.get_str("grpc-status")),
        Some("0"),
        "trailers delivered with END_STREAM must be surfaced through the body, not dropped",
    );
}
