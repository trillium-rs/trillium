use super::fixture::*;
use crate::{Method, h2::H2ErrorCode};
use futures_lite::io::AsyncRead;
use std::{
    future::Future,
    net::Shutdown,
    pin::Pin,
    task::{Context, Poll},
};
use trillium_testing::TestTransport;

/// Reads through everything the driver has written and reports whether the peer then sees the
/// end of the stream, which is how a shut-down sending side looks from the other end.
fn peer_sees_end_of_stream(peer: &mut TestTransport, cx: &mut Context<'_>) -> bool {
    let mut buf = [0; 1024];
    loop {
        match Pin::new(&mut *peer).poll_read(cx, &mut buf) {
            Poll::Ready(Ok(0)) => return true,
            Poll::Ready(Ok(_)) => {}
            Poll::Ready(Err(e)) => panic!("peer read failed: {e}"),
            Poll::Pending => return false,
        }
    }
}

/// The h2spec `generic/3.8` shape: the peer sends GOAWAY and keeps sending. `close` must shut our
/// sending side down first, then consume what the peer sends until the peer closes — dropping the
/// transport with the peer's bytes unread is what turns the close into a TCP reset.
#[test]
fn close_shuts_down_sending_then_waits_for_the_peer() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();
    fx.peer_goaway(0, H2ErrorCode::NoError);
    assert!(
        matches!(fx.tick(), Poll::Ready(None)),
        "a peer GOAWAY with nothing in flight finishes the driver",
    );

    let DriverFixture {
        driver, mut peer, ..
    } = fx;
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut close = Box::pin(driver.close());

    assert!(
        close.as_mut().poll(&mut cx).is_pending(),
        "close waits for the peer to close its side",
    );
    assert!(
        peer_sees_end_of_stream(&mut peer, &mut cx),
        "close shuts down our sending side before it waits",
    );

    peer.write_all([0; 17]);
    assert!(close.as_mut().poll(&mut cx).is_pending());
    assert!(
        peer.write_side().current(),
        "close reads and discards what the peer sends while it waits",
    );

    peer.shutdown(Shutdown::Write);
    assert!(
        close.as_mut().poll(&mut cx).is_ready(),
        "close finishes once the peer closes its side",
    );
}

/// `close` on a driver that hasn't finished ends the connection abruptly, but must still run the
/// teardown that `next` would have: a stream in flight is reset, so a handler parked on it sees the
/// connection end instead of waiting on a driver that no longer exists.
#[test]
fn close_before_finishing_resets_streams_in_flight() {
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

    let DriverFixture { driver, peer, .. } = fx;
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut close = Box::pin(driver.close());

    assert!(close.as_mut().poll(&mut cx).is_pending());
    assert!(
        state.lifecycle_lock().recv_closed(),
        "a stream in flight must be reset when the driver is closed before finishing",
    );

    peer.shutdown(Shutdown::Write);
    assert!(close.as_mut().poll(&mut cx).is_ready());
}
