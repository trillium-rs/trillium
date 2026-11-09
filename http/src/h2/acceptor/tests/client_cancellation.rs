use super::fixture::*;
use crate::{
    Headers, Method, Status,
    h2::{
        H2ErrorCode, H2Transport,
        acceptor::{closed_streams::ClosedStreams, types::DriverState},
        frame::Frame,
    },
    headers::hpack::PseudoHeaders,
};
use futures_lite::io::AsyncRead;
use std::{future::Future, pin::Pin, task::Context};

#[derive(Clone, Copy, Debug)]
enum LateFrame {
    Data,
    Headers,
    Reset,
    WindowUpdate,
}

impl LateFrame {
    fn inject(self, fx: &mut DriverFixture, id: u32) {
        match self {
            Self::Data => fx.peer_data(id, b"late", true),
            Self::Headers => fx.peer_response_headers(id, Status::Ok, true),
            Self::Reset => fx.peer_rst_stream(id, H2ErrorCode::NoError),
            Self::WindowUpdate => fx.peer_window_update(id, 1),
        }
    }
}

pub(super) fn open_get(fx: &mut DriverFixture) -> (u32, H2Transport) {
    let pseudos = PseudoHeaders::default()
        .with_method(Method::Get)
        .with_path("/")
        .with_scheme("https")
        .with_authority("test");
    let (id, _submit, transport) = fx
        .connection
        .open_stream(pseudos, Headers::new(), None)
        .expect("open request");
    assert!(fx.tick().is_pending());
    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|frame| matches!(
            frame,
            Frame::Headers { stream_id, end_stream: true, .. } if *stream_id == id
        )),
        "request HEADERS missing: {frames:?}"
    );
    (id, transport)
}

fn assert_response_headers(fx: &DriverFixture, id: u32) {
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let result = Pin::new(&mut fx.connection.response_headers(id)).poll(&mut cx);
    assert!(
        matches!(result, std::task::Poll::Ready(Ok(_))),
        "response headers on stream {id}: {result:?}"
    );
}

fn late_frame_after_cancellation(late_frame: LateFrame, evict_closed_reason: bool) {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, mut transport) = open_get(&mut fx);

    fx.peer_response_headers(id, Status::Ok, false);
    fx.peer_data(id, b"event: message\ndata: hello\n\n", false);
    assert!(fx.tick().is_pending());
    assert_response_headers(&fx, id);

    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut body = [0; 64];
    let read = Pin::new(&mut transport).poll_read(&mut cx, &mut body);
    let expected_body = b"event: message\ndata: hello\n\n";
    assert!(
        matches!(read, std::task::Poll::Ready(Ok(len)) if len == expected_body.len()),
        "response body chunk: {read:?}"
    );
    assert_eq!(&body[..expected_body.len()], expected_body);

    drop(transport);
    assert!(fx.tick().is_pending());
    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|frame| matches!(
            frame,
            Frame::RstStream { stream_id, error_code: H2ErrorCode::Cancel } if *stream_id == id
        )),
        "cancellation reset missing: {frames:?}"
    );
    assert!(!fx.driver.streams.contains_key(&id));

    if evict_closed_reason {
        for _ in 0..=ClosedStreams::CAP {
            let (_, transport) = open_get(&mut fx);
            drop(transport);
            assert!(fx.tick().is_pending());
            fx.next_outbound_frames();
        }
        assert!(fx.driver.closed_reason(id).is_none());
    }

    let (next_id, _next_transport) = open_get(&mut fx);
    late_frame.inject(&mut fx, id);
    fx.peer_response_headers(next_id, Status::Ok, true);
    let result = fx.tick();
    let frames = fx.next_outbound_frames();
    assert_eq!(
        fx.driver.state,
        DriverState::Running,
        "{late_frame:?} on canceled stream closed the connection: {result:?}, {frames:?}"
    );
    assert_response_headers(&fx, next_id);
}

fn frame_on_unopened_stream(late_frame: LateFrame) {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    late_frame.inject(&mut fx, 1);
    let _ = fx.tick();
    let frames = fx.next_outbound_frames();
    assert!(
        frames.iter().any(|frame| matches!(
            frame,
            Frame::Goaway {
                error_code: H2ErrorCode::ProtocolError,
                ..
            }
        )),
        "{late_frame:?} on unopened stream must produce PROTOCOL_ERROR: {frames:?}"
    );
}

#[test]
fn late_data_after_client_cancellation_keeps_connection_usable() {
    late_frame_after_cancellation(LateFrame::Data, false);
}

#[test]
fn late_headers_after_client_cancellation_keeps_connection_usable() {
    late_frame_after_cancellation(LateFrame::Headers, false);
}

#[test]
fn late_reset_after_client_cancellation_keeps_connection_usable() {
    late_frame_after_cancellation(LateFrame::Reset, false);
}

#[test]
fn late_window_update_after_client_cancellation_keeps_connection_usable() {
    late_frame_after_cancellation(LateFrame::WindowUpdate, false);
}

#[test]
fn data_on_unopened_client_stream_is_connection_error() {
    frame_on_unopened_stream(LateFrame::Data);
}

#[test]
fn headers_on_unopened_client_stream_is_connection_error() {
    frame_on_unopened_stream(LateFrame::Headers);
}

#[test]
fn reset_on_unopened_client_stream_is_connection_error() {
    frame_on_unopened_stream(LateFrame::Reset);
}

#[test]
fn window_update_on_unopened_client_stream_is_connection_error() {
    frame_on_unopened_stream(LateFrame::WindowUpdate);
}

#[test]
fn late_frames_after_close_ledger_eviction_keep_connection_usable() {
    for frame in [
        LateFrame::Data,
        LateFrame::Headers,
        LateFrame::Reset,
        LateFrame::WindowUpdate,
    ] {
        late_frame_after_cancellation(frame, true);
    }
}

#[test]
fn even_and_future_stream_ids_remain_idle_after_opening_client_streams() {
    for frame in [
        LateFrame::Data,
        LateFrame::Headers,
        LateFrame::Reset,
        LateFrame::WindowUpdate,
    ] {
        for id in [2, 5] {
            let mut fx = DriverFixture::new_client();
            fx.complete_handshake_client();
            let (_, _first) = open_get(&mut fx);
            let (_, _second) = open_get(&mut fx);
            frame.inject(&mut fx, id);
            let _ = fx.tick();
            let frames = fx.next_outbound_frames();
            assert!(
                frames.iter().any(|frame| matches!(
                    frame,
                    Frame::Goaway {
                        error_code: H2ErrorCode::ProtocolError,
                        ..
                    }
                )),
                "{frame:?} on idle stream {id}: {frames:?}"
            );
        }
    }
}

fn cancel_before_opening(fx: &mut DriverFixture) -> u32 {
    let pseudos = PseudoHeaders::default()
        .with_method(Method::Get)
        .with_path("/")
        .with_scheme("https")
        .with_authority("test");
    let (id, _submit, transport) = fx
        .connection
        .open_stream(pseudos, Headers::new(), None)
        .expect("allocate request");
    drop(transport);
    assert!(fx.tick().is_pending());
    let frames = fx.next_outbound_frames();
    assert!(!frames.iter().any(|frame| matches!(frame,
        Frame::Headers { stream_id, .. } | Frame::RstStream { stream_id, .. } if *stream_id == id
    )), "canceled idle stream reached the wire: {frames:?}");
    assert!(!fx.driver.streams.contains_key(&id));
    id
}

#[test]
fn cancellation_before_opening_sends_no_reset_and_next_request_works() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let canceled = cancel_before_opening(&mut fx);
    let (next, _transport) = open_get(&mut fx);
    assert!(next > canceled);
    fx.peer_response_headers(next, Status::Ok, true);
    assert!(fx.tick().is_pending());
    assert_response_headers(&fx, next);
}

#[test]
fn allocated_but_unsent_client_stream_is_still_idle() {
    for frame in [
        LateFrame::Data,
        LateFrame::Headers,
        LateFrame::Reset,
        LateFrame::WindowUpdate,
    ] {
        let mut fx = DriverFixture::new_client();
        fx.complete_handshake_client();
        let id = cancel_before_opening(&mut fx);
        frame.inject(&mut fx, id);
        let _ = fx.tick();
        let frames = fx.next_outbound_frames();
        assert!(
            frames.iter().any(|frame| matches!(
                frame,
                Frame::Goaway {
                    error_code: H2ErrorCode::ProtocolError,
                    ..
                }
            )),
            "{frame:?} on allocated but unsent stream: {frames:?}"
        );
    }
}
