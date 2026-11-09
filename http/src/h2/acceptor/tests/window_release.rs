//! Received DATA that no handler will read must still return its connection-level window to the
//! peer. Otherwise each discarded or abandoned byte shrinks the window for every later stream, and
//! enough of them stall the connection.

use super::{client_cancellation::open_get, fixture::*};
use crate::{
    Headers, KnownHeaderName, Method, Status,
    h2::{
        H2ErrorCode, H2Transport,
        frame::{Frame, data as data_frame},
    },
    headers::hpack::PseudoHeaders,
};
use futures_lite::io::AsyncRead;
use std::{
    pin::Pin,
    task::{Context, Poll},
};

const BODY: &[u8] = &[b'x'; 5000];

/// Total `WINDOW_UPDATE` increment granted on `id` (0 for the connection).
fn window_credit(frames: &[Frame], id: u32) -> u64 {
    frames
        .iter()
        .filter_map(|frame| match frame {
            Frame::WindowUpdate {
                stream_id,
                increment,
            } if *stream_id == id => Some(u64::from(*increment)),
            _ => None,
        })
        .sum()
}

/// Tick enough times to process everything the peer wrote, returning every frame the driver wrote.
/// A `Pending` tick can be a cooperative yield with input still unread, so it can't end the loop.
fn settle(fx: &mut DriverFixture) -> Vec<Frame> {
    let mut frames = Vec::new();
    for _ in 0..8 {
        let _ = fx.tick();
        frames.extend(fx.next_outbound_frames());
    }
    frames
}

fn peer_padded_data(fx: &mut DriverFixture, stream_id: u32, data: &[u8], padding: u8) {
    let prefix_len = data_frame::encoded_prefix_len(padding);
    let mut frame = vec![0; prefix_len + data.len() + usize::from(padding)];
    let data_len = u32::try_from(data.len()).expect("data fits u32");
    data_frame::encode_prefix(stream_id, false, data_len, padding, &mut frame)
        .expect("encode DATA prefix");
    frame[prefix_len..prefix_len + data.len()].copy_from_slice(data);
    fx.peer.write_all(&frame);
}

/// Read up to `len` bytes of response body, returning how many arrived.
fn read_body(transport: &mut H2Transport, len: usize) -> usize {
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut buf = vec![0; len];
    match Pin::new(transport).poll_read(&mut cx, &mut buf) {
        Poll::Ready(Ok(read)) => read,
        other => panic!("body read: {other:?}"),
    }
}

/// A client stream whose response headers and `body` have arrived, unread.
fn client_with_buffered_body(body: &[u8]) -> (DriverFixture, u32, H2Transport) {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, transport) = open_get(&mut fx);
    fx.peer_response_headers(id, Status::Ok, false);
    fx.peer_data(id, body, false);
    settle(&mut fx);
    (fx, id, transport)
}

#[test]
fn padding_is_credited_without_reads() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();
    fx.peer_open_stream(1, Method::Post, "/", false);
    let Poll::Ready(Some(Ok(_conn))) = fx.tick() else {
        panic!("expected a Conn");
    };
    settle(&mut fx);

    // Sixteen frames of 100 body bytes plus 1 pad-length byte and 255 padding: 4096 padding bytes,
    // enough to clear the WINDOW_UPDATE batching threshold. The unread body bytes stay held.
    for _ in 0..16 {
        peer_padded_data(&mut fx, 1, &[b'x'; 100], 255);
    }
    let frames = settle(&mut fx);
    assert_eq!(window_credit(&frames, 0), 4096, "{frames:?}");
    assert_eq!(window_credit(&frames, 1), 4096, "{frames:?}");
}

#[test]
fn late_data_after_client_cancellation_is_credited() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, transport) = open_get(&mut fx);
    fx.peer_response_headers(id, Status::Ok, false);
    settle(&mut fx);
    drop(transport);
    settle(&mut fx);

    fx.peer_data(id, BODY, true);
    let frames = settle(&mut fx);
    assert_eq!(window_credit(&frames, 0), 5000, "{frames:?}");
}

#[test]
fn unread_response_body_is_credited_when_client_cancels() {
    let (mut fx, _, transport) = client_with_buffered_body(BODY);
    drop(transport);
    let frames = settle(&mut fx);
    assert_eq!(window_credit(&frames, 0), 5000, "{frames:?}");
}

/// Half the body is read before the cancel: the read half is credited through consumption (already
/// serviced, or still pending when the stream leaves), the rest at removal, each byte once.
#[test]
fn partial_read_then_cancel_credits_each_byte_once() {
    for serviced in [true, false] {
        let (mut fx, _, mut transport) = client_with_buffered_body(&[BODY, BODY].concat());
        assert_eq!(read_body(&mut transport, BODY.len()), BODY.len());
        let mut frames = Vec::new();
        if serviced {
            frames.extend(settle(&mut fx));
        }
        drop(transport);
        frames.extend(settle(&mut fx));
        assert_eq!(
            window_credit(&frames, 0),
            10_000,
            "serviced={serviced}: {frames:?}"
        );
    }
}

/// Removal credits everything buffered, so reading those bytes afterward credits nothing more.
#[test]
fn reads_after_peer_reset_add_no_credit() {
    let (mut fx, id, mut transport) = client_with_buffered_body(BODY);
    fx.peer_rst_stream(id, H2ErrorCode::Cancel);
    let frames = settle(&mut fx);
    assert_eq!(window_credit(&frames, 0), 5000, "{frames:?}");

    let _ = read_body(&mut transport, BODY.len());
    let frames = settle(&mut fx);
    assert_eq!(window_credit(&frames, 0), 0, "{frames:?}");
}

#[test]
fn unread_request_body_is_credited_when_handler_drops_conn() {
    let mut fx = DriverFixture::new_server();
    fx.complete_handshake();
    fx.peer_open_stream(1, Method::Post, "/", false);
    let Poll::Ready(Some(Ok(conn))) = fx.tick() else {
        panic!("expected a Conn");
    };
    fx.peer_data(1, BODY, false);
    settle(&mut fx);

    drop(conn);
    let frames = settle(&mut fx);
    assert_eq!(window_credit(&frames, 0), 5000, "{frames:?}");
}

#[test]
fn data_rejected_on_open_stream_is_credited() {
    for content_length_mismatch in [false, true] {
        let mut fx = DriverFixture::new_client();
        fx.complete_handshake_client();
        let (id, _transport) = open_get(&mut fx);
        let mut headers = Headers::new();
        if content_length_mismatch {
            headers.insert(KnownHeaderName::ContentLength, "6000");
        }
        fx.peer_headers(
            id,
            PseudoHeaders::default().with_status(Status::Ok),
            &headers,
            false,
        );
        fx.peer_data(id, BODY, !content_length_mismatch);
        settle(&mut fx);

        // Past the declared length, or after END_STREAM: rejected without reaching the ring. The
        // first frame's buffered bytes are released along with the stream.
        fx.peer_data(id, BODY, false);
        let frames = settle(&mut fx);
        assert_eq!(
            window_credit(&frames, 0),
            10_000,
            "content_length_mismatch={content_length_mismatch}: {frames:?}"
        );
    }
}
