//! A response body that fails mid-send must still close its stream on the wire. Otherwise the peer
//! waits on the stream indefinitely, holding one of its concurrent-stream slots.

use super::fixture::*;
use crate::{
    Body, Headers, Method, Status,
    h2::{H2ErrorCode, SubmitSend, frame::Frame},
    headers::hpack::PseudoHeaders,
};
use futures_lite::io::AsyncRead;
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
};

/// Yields `data` in one read, then `end`: an error, or `None` for EOF.
struct ThenEnds {
    data: &'static [u8],
    sent: bool,
    end: Option<io::ErrorKind>,
}

impl AsyncRead for ThenEnds {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if !self.sent {
            self.sent = true;
            buf[..self.data.len()].copy_from_slice(self.data);
            return Poll::Ready(Ok(self.data.len()));
        }
        Poll::Ready(match self.end {
            Some(kind) => Err(io::Error::new(kind, "body source failed")),
            None => Ok(0),
        })
    }
}

/// Submit `body` as stream 1's response and tick until the driver has dealt with it.
fn respond_with(body: Body) -> (DriverFixture, SubmitSend, Vec<Frame>) {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();
    fx.peer_open_stream(1, Method::Get, "/", true);
    let Poll::Ready(Some(Ok(_conn))) = fx.tick() else {
        panic!("expected a Conn");
    };
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let submit = fx
        .connection
        .submit_send(1, pseudos, Headers::new(), Some(body));
    let mut frames = Vec::new();
    for _ in 0..4 {
        let _ = fx.tick();
        frames.extend(fx.next_outbound_frames());
    }
    (fx, submit, frames)
}

fn assert_reset_without_end_stream(fx: &DriverFixture, submit: &mut SubmitSend, frames: &[Frame]) {
    assert!(
        frames.iter().any(|frame| matches!(
            frame,
            Frame::RstStream {
                stream_id: 1,
                error_code: H2ErrorCode::InternalError
            }
        )),
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
        "a failed body must not end the stream cleanly: {frames:?}"
    );
    assert!(!fx.connection.streams_lock().contains_key(&1));

    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(
        matches!(Pin::new(submit).poll(&mut cx), Poll::Ready(Err(_))),
        "the handler's send should report the failure"
    );
}

#[test]
fn body_error_resets_the_stream() {
    let body = Body::new_streaming(
        ThenEnds {
            data: b"partial",
            sent: false,
            end: Some(io::ErrorKind::ConnectionReset),
        },
        None,
    );
    let (fx, mut submit, frames) = respond_with(body);
    assert_reset_without_end_stream(&fx, &mut submit, &frames);
}
