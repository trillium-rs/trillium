//! A body that fails mid-send — an error, or EOF short of its declared length — must still close
//! its stream on the wire, and not with a clean `END_STREAM`. Otherwise the peer waits on the
//! stream indefinitely, or takes a truncated body as complete. Streams that already closed must not
//! be reset again.

use super::fixture::*;
use crate::{
    Body, BodySource, Headers, Method, Status,
    h2::{H2ErrorCode, SubmitSend, frame::Frame, transport::StreamState},
    headers::hpack::PseudoHeaders,
};
use futures_lite::io::AsyncRead;
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
};

/// Yields `data` in one read (EOF at once if it's empty), then `end`: an error, or `None` for EOF.
struct ThenEnds {
    data: &'static [u8],
    sent: bool,
    end: Option<io::ErrorKind>,
}

impl ThenEnds {
    fn body(data: &'static [u8], end: Option<io::ErrorKind>, declared_len: Option<u64>) -> Body {
        Body::new_streaming(
            Self {
                data,
                sent: false,
                end,
            },
            declared_len,
        )
    }
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

/// Every frame the driver writes over a few ticks.
fn settle(fx: &mut DriverFixture) -> Vec<Frame> {
    let mut frames = Vec::new();
    for _ in 0..4 {
        let _ = fx.tick();
        frames.extend(fx.next_outbound_frames());
    }
    frames
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
    let frames = settle(&mut fx);
    (fx, submit, frames)
}

fn assert_reset_without_end_stream(
    fx: &DriverFixture,
    id: u32,
    submit: &mut SubmitSend,
    frames: &[Frame],
) {
    assert_eq!(
        resets_on(frames, id),
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
        "a failed body must not end the stream cleanly: {frames:?}"
    );
    assert!(!fx.connection.streams_lock().contains_key(&id));

    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    assert!(
        matches!(Pin::new(submit).poll(&mut cx), Poll::Ready(Err(_))),
        "the send should report the failure"
    );
}

#[test]
fn body_error_resets_the_stream() {
    let body = ThenEnds::body(b"partial", Some(io::ErrorKind::ConnectionReset), None);
    let (fx, mut submit, frames) = respond_with(body);
    assert_reset_without_end_stream(&fx, 1, &mut submit, &frames);
}

#[test]
fn body_ending_before_its_declared_length_resets_the_stream() {
    for data in [&b"partial"[..], b""] {
        let (fx, mut submit, frames) = respond_with(ThenEnds::body(data, None, Some(100)));
        assert_reset_without_end_stream(&fx, 1, &mut submit, &frames);
    }
}

/// A waker that records, each time it fires, whether the stream had already closed.
struct ClosedAtWake {
    state: Arc<StreamState>,
    observed: Mutex<Vec<bool>>,
}

impl Wake for ClosedAtWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let closed = self.state.lifecycle_lock().is_closed();
        self.observed.lock().unwrap().push(closed);
    }
}

/// A client request body that fails resets the stream, and the reset lands before the response
/// waiter is woken: a waiter that re-checked an open stream would park with nothing left to wake
/// it.
#[test]
fn request_body_error_resets_the_stream_before_waking_waiters() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let pseudos = PseudoHeaders::default()
        .with_method(Method::Post)
        .with_path("/")
        .with_scheme("https")
        .with_authority("test");
    let body = ThenEnds::body(b"partial", Some(io::ErrorKind::ConnectionReset), None);
    let (id, mut submit, _transport) = fx
        .connection
        .open_stream(pseudos, Headers::new(), Some(body))
        .expect("open request");

    let state = fx.connection.streams_lock()[&id].clone();
    let observer = Arc::new(ClosedAtWake {
        state,
        observed: Mutex::new(Vec::new()),
    });
    let waker = Waker::from(observer.clone());
    let mut cx = Context::from_waker(&waker);
    assert!(
        Pin::new(&mut fx.connection.response_headers(id))
            .poll(&mut cx)
            .is_pending()
    );

    let frames = settle(&mut fx);
    assert_reset_without_end_stream(&fx, id, &mut submit, &frames);
    let observed = observer.observed.lock().unwrap();
    assert!(
        !observed.is_empty() && observed.iter().all(|closed| *closed),
        "response waiter woken before the stream closed: {observed:?}"
    );
}

#[derive(Clone, Copy, Debug)]
enum Teardown {
    CleanClose,
    PeerReset,
    HandlerDrop,
}

/// Streams that close on the wire before removal draw no extra reset from the open-stream guard.
#[test]
fn closed_streams_are_not_reset_again() {
    for teardown in [
        Teardown::CleanClose,
        Teardown::PeerReset,
        Teardown::HandlerDrop,
    ] {
        let mut fx = DriverFixture::new_server();
        fx.complete_handshake();
        let end_request = matches!(teardown, Teardown::CleanClose);
        fx.peer_open_stream(1, Method::Post, "/", end_request);
        let Poll::Ready(Some(Ok(conn))) = fx.tick() else {
            panic!("expected a Conn");
        };
        let mut conn = Some(conn);
        let expected = match teardown {
            Teardown::CleanClose => {
                let pseudos = PseudoHeaders::default().with_status(Status::Ok);
                let body = ThenEnds::body(b"ok", None, None);
                let _submit = fx
                    .connection
                    .submit_send(1, pseudos, Headers::new(), Some(body));
                vec![]
            }
            Teardown::PeerReset => {
                fx.peer_rst_stream(1, H2ErrorCode::Cancel);
                vec![]
            }
            Teardown::HandlerDrop => {
                drop(conn.take());
                vec![H2ErrorCode::Cancel]
            }
        };
        let frames = settle(&mut fx);
        assert_eq!(resets_on(&frames, 1), expected, "{teardown:?}: {frames:?}");
        assert!(
            !fx.connection.streams_lock().contains_key(&1),
            "{teardown:?}"
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PanicAt {
    Read,
    Trailers,
    Drop,
    ReadAndDrop,
}

/// Yields `partial`, then EOF — panicking instead wherever `at` says.
struct Panicking {
    at: PanicAt,
    sent: bool,
}

impl Panicking {
    fn body(at: PanicAt) -> Body {
        Body::new_with_trailers(Self { at, sent: false }, None)
    }
}

impl AsyncRead for Panicking {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if !self.sent {
            self.sent = true;
            buf[..7].copy_from_slice(b"partial");
            return Poll::Ready(Ok(7));
        }
        assert!(
            !matches!(self.at, PanicAt::Read | PanicAt::ReadAndDrop),
            "body source panicked in poll_read"
        );
        Poll::Ready(Ok(0))
    }
}

impl BodySource for Panicking {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        assert_ne!(
            self.at,
            PanicAt::Trailers,
            "body source panicked in trailers"
        );
        None
    }
}

impl Drop for Panicking {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert!(
                !matches!(self.at, PanicAt::Drop | PanicAt::ReadAndDrop),
                "body source panicked in drop"
            );
        }
    }
}

/// After a body panic, the connection still serves another stream end to end.
fn assert_connection_still_serves(fx: &mut DriverFixture, context: PanicAt) {
    fx.peer_open_stream(3, Method::Get, "/", true);
    let Poll::Ready(Some(Ok(_conn))) = fx.tick() else {
        panic!("{context:?}: the driver stopped accepting streams");
    };
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let _submit = fx.connection.submit_send(
        3,
        pseudos,
        Headers::new(),
        Some(ThenEnds::body(b"ok", None, None)),
    );
    let frames = settle(fx);
    assert!(
        frames.iter().any(|frame| matches!(
            frame,
            Frame::Data {
                stream_id: 3,
                end_stream: true,
                ..
            }
        )),
        "{context:?}: the next response didn't complete: {frames:?}"
    );
    assert_eq!(resets_on(&frames, 3), [], "{context:?}: {frames:?}");
}

/// A body source that panics resets its own stream, or — panicking only in its drop, after it
/// was fully sent — none at all. Either way the driver keeps serving the connection's other
/// streams.
#[test]
fn body_panics_cost_only_their_stream() {
    for at in [
        PanicAt::Read,
        PanicAt::Trailers,
        PanicAt::Drop,
        PanicAt::ReadAndDrop,
    ] {
        let (mut fx, mut submit, frames) = respond_with(Panicking::body(at));
        if at == PanicAt::Drop {
            assert_eq!(resets_on(&frames, 1), [], "{at:?}: {frames:?}");
            let waker = noop_waker();
            let mut cx = Context::from_waker(&waker);
            assert!(
                matches!(Pin::new(&mut submit).poll(&mut cx), Poll::Ready(Ok(()))),
                "{at:?}"
            );
        } else {
            assert_reset_without_end_stream(&fx, 1, &mut submit, &frames);
        }
        assert_connection_still_serves(&mut fx, at);
    }
}

/// A panicking client request body resets its own stream; the connection's next request still
/// goes out.
#[test]
fn request_body_panic_costs_only_its_stream() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let pseudos = PseudoHeaders::default()
        .with_method(Method::Post)
        .with_path("/")
        .with_scheme("https")
        .with_authority("test");
    let (id, mut submit, _transport) = fx
        .connection
        .open_stream(
            pseudos,
            Headers::new(),
            Some(Panicking::body(PanicAt::Read)),
        )
        .expect("open request");
    let frames = settle(&mut fx);
    assert_reset_without_end_stream(&fx, id, &mut submit, &frames);

    let (next, _transport) = super::client_cancellation::open_get(&mut fx);
    assert!(next > id);
}
