//! A handler that panics drops its conn while unwinding. On a server, that's a failure the peer
//! should hear about as one: a `500` if no response was staged, else `RST_STREAM(INTERNAL_ERROR)`.
//! A client abandoning its stream is a cancellation whatever the reason.

use super::{client_cancellation::open_get, fixture::*};
use crate::{
    Body, Headers, Method, Status,
    h2::{H2ErrorCode, frame::Frame},
    headers::hpack::PseudoHeaders,
};
use futures_lite::io::AsyncRead;
use std::{
    future::Future,
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    task::{Context, Poll},
};

/// Drop `held` while a panic unwinds, as a panicking handler drops its conn.
pub(super) fn panic_holding<T>(held: T) {
    let result = catch_unwind(AssertUnwindSafe(move || {
        let _held = held;
        panic!("handler panicked");
    }));
    assert!(result.is_err());
}

fn resets_on(frames: &[Frame], id: u32) -> Vec<H2ErrorCode> {
    frames
        .iter()
        .filter_map(|frame| match frame {
            Frame::RstStream {
                stream_id,
                error_code,
            } if *stream_id == id => Some(*error_code),
            _ => None,
        })
        .collect()
}

/// A body that never yields, keeping its response in flight.
struct NeverReady;

impl AsyncRead for NeverReady {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Pending
    }
}

#[test]
fn handler_panic_mid_response_resets_with_internal_error() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();
    fx.peer_open_stream(1, Method::Get, "/", true);
    let Poll::Ready(Some(Ok(conn))) = fx.tick() else {
        panic!("expected a Conn");
    };

    let mut send = Box::pin(
        conn.with_status(Status::Ok)
            .with_response_body(Body::new_streaming(NeverReady, None))
            .send_h2(),
    );
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(send.as_mut().poll(&mut cx).is_pending());
    let _ = fx.tick();
    let frames = fx.next_outbound_frames();
    assert!(
        frames
            .iter()
            .any(|frame| matches!(frame, Frame::Headers { stream_id: 1, .. })),
        "the response should be underway: {frames:?}"
    );

    panic_holding(send);
    let _ = fx.tick();
    let frames = fx.next_outbound_frames();
    assert_eq!(
        resets_on(&frames, 1),
        [H2ErrorCode::InternalError],
        "{frames:?}"
    );
}

/// The threshold is staging, not the wire: a response the handler staged but the driver hasn't
/// framed yet still counts as underway.
#[test]
fn handler_panic_after_staging_resets_with_internal_error() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();
    fx.peer_open_stream(1, Method::Get, "/", true);
    let Poll::Ready(Some(Ok(conn))) = fx.tick() else {
        panic!("expected a Conn");
    };
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_send(1, pseudos, Headers::new(), None);

    panic_holding(conn);
    let _ = fx.tick();
    let frames = fx.next_outbound_frames();
    assert_eq!(
        resets_on(&frames, 1),
        [H2ErrorCode::InternalError],
        "{frames:?}"
    );
}

/// An upgrade handler that panics resets its tunnel instead of closing it gracefully.
#[test]
fn upgrade_handler_panic_resets_with_internal_error() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();
    fx.peer_open_stream(1, Method::Get, "/", true);
    let Poll::Ready(Some(Ok(conn))) = fx.tick() else {
        panic!("expected a Conn");
    };
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let mut submit = fx
        .connection
        .submit_upgrade(1, pseudos, Headers::new(), None);
    let _ = fx.tick();
    let _ = fx.tick();
    fx.next_outbound_frames();
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(
        matches!(Pin::new(&mut submit).poll(&mut cx), Poll::Ready(Ok(()))),
        "the upgrade should be handed off"
    );

    panic_holding(conn);
    let _ = fx.tick();
    let frames = fx.next_outbound_frames();
    assert_eq!(
        resets_on(&frames, 1),
        [H2ErrorCode::InternalError],
        "{frames:?}"
    );
    assert!(
        !frames.iter().any(|frame| matches!(
            frame,
            Frame::Data {
                end_stream: true,
                ..
            }
        )),
        "a panicking tunnel must not close gracefully: {frames:?}"
    );
}

#[test]
fn client_panic_cancels_its_stream() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, transport) = open_get(&mut fx);
    fx.peer_response_headers(id, Status::Ok, false);
    let _ = fx.tick();
    fx.next_outbound_frames();

    panic_holding(transport);
    let _ = fx.tick();
    let frames = fx.next_outbound_frames();
    assert_eq!(resets_on(&frames, id), [H2ErrorCode::Cancel], "{frames:?}");
}
