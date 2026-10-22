//! Conn-task → driver signaling: work a handler hands the driver after the driver last looked
//! must keep `park` from sleeping. These assert `park`'s decision directly, because a following
//! `tick` would service the work either way and hide a lost wakeup.

use super::fixture::*;
use crate::{Headers, Method, Status, h2::frame::Frame, headers::hpack::PseudoHeaders};
use futures_lite::io::AsyncRead;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

/// Whether the driver would sleep now, rather than loop around for more work.
fn parks(fx: &mut DriverFixture) -> bool {
    let waker = noop_waker();
    fx.driver.park(&mut Context::from_waker(&waker))
}

#[test]
fn a_response_submitted_while_the_driver_sleeps_is_framed() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();
    fx.peer_open_stream(1, Method::Get, "/", true);
    let _conn = match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!("expected Conn yielded for stream 1, got {other:?}"),
    };
    assert!(fx.tick().is_pending());
    assert!(parks(&mut fx), "nothing has been submitted yet");

    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_send(1, pseudos, Headers::new(), None);
    assert!(
        !parks(&mut fx),
        "a submitted response must keep the driver awake"
    );

    let _ = fx.tick();
    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::Headers {
                stream_id: 1,
                end_stream: true,
                ..
            }
        )),
        "expected the response HEADERS(END_STREAM) on stream 1, got {frames:?}",
    );
}

#[test]
fn a_ping_queued_while_the_driver_sleeps_is_sent() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();
    assert!(fx.tick().is_pending());

    let connection = fx.connection.clone();
    let _ping = connection.send_ping(*b"pingpong");
    assert!(!parks(&mut fx), "a queued PING must keep the driver awake");

    let _ = fx.tick();
    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::Ping {
                opaque_data,
                ack: false,
            } if opaque_data == b"pingpong"
        )),
        "expected the queued PING, got {frames:?}",
    );
}

/// A client stream whose handler signals before the driver has picked the stream up (here, by
/// reading the response body at once) is serviced exactly once: its request HEADERS go out once,
/// and its read intent still earns a WINDOW_UPDATE.
#[test]
fn a_client_stream_signaled_before_pickup_is_serviced_once() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();

    let pseudos = PseudoHeaders::default()
        .with_method(Method::Get)
        .with_path("/")
        .with_scheme("http")
        .with_authority("test");
    let (id, _submit, mut transport) = fx
        .connection
        .open_stream(pseudos, Headers::new(), None)
        .expect("open_stream on a running client connection");
    let waker = noop_waker();
    let mut buf = [0; 8];
    assert!(
        Pin::new(&mut transport)
            .poll_read(&mut Context::from_waker(&waker), &mut buf)
            .is_pending(),
        "no response yet",
    );
    assert!(!parks(&mut fx), "a new stream must keep the driver awake");

    let _ = fx.tick();
    let frames = fx.next_outbound_frames();
    let headers = frames
        .iter()
        .filter(|f| matches!(f, Frame::Headers { stream_id, .. } if *stream_id == id))
        .count();
    assert_eq!(headers, 1, "request HEADERS framed once; got {frames:?}");
    assert!(
        frames
            .iter()
            .any(|f| matches!(f, Frame::WindowUpdate { stream_id, .. } if *stream_id == id)),
        "read intent should raise stream {id}'s window; got {frames:?}",
    );
}
