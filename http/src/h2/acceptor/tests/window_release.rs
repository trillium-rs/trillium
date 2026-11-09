//! Received DATA that no handler will read must still return its connection-level window to the
//! peer. Otherwise each discarded or abandoned byte shrinks the window for every later stream, and
//! enough of them stall the connection.

use super::{client_cancellation::open_get, fixture::*};
use crate::{
    Headers, KnownHeaderName, Method, Status,
    h2::frame::{Frame, data as data_frame},
    headers::hpack::PseudoHeaders,
};
use std::task::Poll;

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

fn peer_padded_data(fx: &mut DriverFixture, stream_id: u32, padding: u8) {
    let mut frame = vec![0; data_frame::encoded_prefix_len(padding) + usize::from(padding)];
    data_frame::encode_prefix(stream_id, false, 0, padding, &mut frame)
        .expect("encode DATA prefix");
    fx.peer.write_all(&frame);
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

    // Sixteen padding-only frames: 16 × (1 pad-length byte + 255 padding) = 4096 bytes, enough to
    // clear the WINDOW_UPDATE batching threshold.
    for _ in 0..16 {
        peer_padded_data(&mut fx, 1, 255);
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
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (id, transport) = open_get(&mut fx);
    fx.peer_response_headers(id, Status::Ok, false);
    fx.peer_data(id, BODY, false);
    settle(&mut fx);

    drop(transport);
    let frames = settle(&mut fx);
    assert_eq!(window_credit(&frames, 0), 5000, "{frames:?}");
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
        if content_length_mismatch {
            let mut headers = Headers::new();
            headers.insert(KnownHeaderName::ContentLength, "4");
            fx.peer_headers(
                id,
                PseudoHeaders::default().with_status(Status::Ok),
                &headers,
                false,
            );
        } else {
            fx.peer_response_headers(id, Status::Ok, true);
        }
        settle(&mut fx);

        // Past the declared length, or after END_STREAM: rejected without reaching the ring.
        fx.peer_data(id, BODY, false);
        let frames = settle(&mut fx);
        assert_eq!(
            window_credit(&frames, 0),
            5000,
            "content_length_mismatch={content_length_mismatch}: {frames:?}"
        );
    }
}
