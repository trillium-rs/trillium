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
    Headers, Method, Status,
    h2::{H2ErrorCode, H2Transport, SubmitSend, frame::Frame},
};
use std::{
    future::Future,
    net::Shutdown,
    pin::Pin,
    task::{Context, Poll},
};

/// Open a body-less GET on a freshly-handshaked client fixture, returning the stream id and
/// the held `SubmitSend` + `H2Transport` (kept alive by the caller so the stream isn't
/// torn down by a dropped transport). Asserts the request HEADERS(END_STREAM) is framed.
fn open_get(fx: &mut DriverFixture) -> (u32, SubmitSend, H2Transport) {
    use crate::headers::hpack::PseudoHeaders;
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
        other => panic!("response_headers should resolve Ok after the peer's HEADERS; got {other:?}"),
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
