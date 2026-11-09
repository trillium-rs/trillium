//! A header block the driver rejects at stream level must still be HPACK-decoded: it can update
//! the connection-wide dynamic table, and skipping that update desynchronizes every later block.
//! Each test sends a rejected block that inserts a dynamic-table entry, then a live block that
//! references the entry by index.

use super::{client_cancellation::open_get, fixture::*};
use crate::{
    Conn, HttpConfig, Method,
    h2::{
        H2ErrorCode, H2Transport,
        frame::{Frame, PriorityInfo, headers as headers_frame},
    },
};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

/// Dynamic table size update to 4096 (the default). The fixture's peer encoder shrinks the
/// driver's table to 0 in its first block, so blocks that follow one of its frames restore the size
/// before inserting. A size update is legal only at the start of a block.
const RESTORE_TABLE_SIZE: &[u8] = &[0x3f, 0xe1, 0x1f];

/// `x-sync: probe` as a literal with incremental indexing (new name). It becomes dynamic-table
/// index 62, the first index past the 61-entry static table.
const INSERT_SYNC: &[u8] = b"\x40\x06x-sync\x05probe";

/// Indexed field referencing dynamic-table index 62.
const USE_SYNC: &[u8] = &[0x80 | 62];

/// `:method: GET`, `:scheme: http`, `:path: /` (static indices 2, 6, 4), then `:authority: test`
/// as a literal without indexing on static name index 1.
const GET_REQUEST: &[u8] = b"\x82\x86\x84\x01\x04test";

/// `:status: 200` (static index 8).
const STATUS_OK: &[u8] = &[0x88];

/// `:method: GET` alone; prefixed to [`GET_REQUEST`] it duplicates the pseudo-header.
const METHOD_GET: &[u8] = &[0x82];

/// Indexed field referencing dynamic-table index 63, past any entry these tests insert.
const UNKNOWN_INDEX: &[u8] = &[0x80 | 63];

/// Write a HEADERS frame (`END_STREAM` set) carrying `fragment` raw header-block bytes.
/// Without `end_headers`, the block continues in CONTINUATION frames.
fn peer_headers_fragment(
    fx: &mut DriverFixture,
    stream_id: u32,
    fragment: &[u8],
    priority: Option<PriorityInfo>,
    end_headers: bool,
) {
    let prefix_len = headers_frame::encoded_prefix_len(0, priority.is_some());
    let mut frame = vec![0; prefix_len + fragment.len()];
    let len = u32::try_from(fragment.len()).expect("fragment fits u32");
    headers_frame::encode_prefix(stream_id, true, end_headers, priority, len, 0, &mut frame)
        .expect("encode HEADERS prefix");
    frame[prefix_len..].copy_from_slice(fragment);
    fx.peer.write_all(&frame);
}

#[derive(Clone, Copy, Debug)]
enum Delivery {
    /// One HEADERS frame.
    Whole,
    /// HEADERS plus one CONTINUATION, written together.
    Split,
    /// HEADERS, then a driver tick and the test's state change, then CONTINUATION.
    Interleaved,
}

/// Write `block` on `stream_id` as `delivery` says, running `between` after the first fragment
/// when interleaved. Returns every frame the driver wrote.
fn deliver_block(
    fx: &mut DriverFixture,
    stream_id: u32,
    block: &[u8],
    priority: Option<PriorityInfo>,
    delivery: Delivery,
    between: impl FnOnce(&mut DriverFixture),
) -> Vec<Frame> {
    if matches!(delivery, Delivery::Whole) {
        peer_headers_fragment(fx, stream_id, block, priority, true);
        let _ = fx.tick();
        return fx.next_outbound_frames();
    }
    let (head, tail) = block.split_at(block.len() / 2);
    peer_headers_fragment(fx, stream_id, head, priority, false);
    if matches!(delivery, Delivery::Interleaved) {
        assert!(fx.tick().is_pending());
        between(fx);
        assert!(fx.tick().is_pending());
    }
    fx.peer_continuation(stream_id, tail, true);
    let _ = fx.tick();
    fx.next_outbound_frames()
}

fn assert_no_goaway(frames: &[Frame]) {
    assert_eq!(count_goaways(frames), 0, "connection closed: {frames:?}");
}

fn rst_on(frames: &[Frame], id: u32) -> Option<H2ErrorCode> {
    frames.iter().find_map(|frame| match frame {
        Frame::RstStream {
            stream_id,
            error_code,
        } if *stream_id == id => Some(*error_code),
        _ => None,
    })
}

fn assert_rst(frames: &[Frame], id: u32, code: H2ErrorCode) {
    assert_eq!(rst_on(frames, id), Some(code), "stream {id}: {frames:?}");
}

#[derive(Clone, Copy, Debug)]
enum ServerRejection {
    /// Over the concurrent-stream limit.
    Refused,
    /// HEADERS on a stream the peer already reset.
    Reset,
    /// HEADERS whose priority names its own stream.
    SelfDependent,
    /// A request block with a duplicated pseudo-header.
    Malformed,
}

/// Setup for a server block rejection: the rejected stream id, the reset code it should draw, its
/// HEADERS priority, a request block prefix that makes it malformed, and a `Conn` occupying the
/// single concurrent-stream slot (dropped once the rejected block is in).
struct Rejection {
    id: u32,
    code: H2ErrorCode,
    priority: Option<PriorityInfo>,
    prefix: &'static [u8],
    occupant: Option<Conn<H2Transport>>,
}

fn expect_conn(fx: &mut DriverFixture) -> Conn<H2Transport> {
    match fx.tick() {
        Poll::Ready(Some(Ok(conn))) => conn,
        other => panic!(
            "expected a Conn, got {other:?}: {:?}",
            fx.next_outbound_frames()
        ),
    }
}

fn server_with_rejection(rejection: ServerRejection) -> (DriverFixture, Rejection) {
    let mut fx = DriverFixture::new_server_with_config(
        HttpConfig::default().with_h2_max_concurrent_streams(1),
    );
    fx.complete_handshake();
    let mut setup = Rejection {
        id: 1,
        code: H2ErrorCode::ProtocolError,
        priority: None,
        prefix: &[],
        occupant: None,
    };
    match rejection {
        ServerRejection::Refused => {
            fx.peer_open_stream(1, Method::Get, "/", true);
            setup.occupant = Some(expect_conn(&mut fx));
            setup.id = 3;
            setup.code = H2ErrorCode::RefusedStream;
        }
        ServerRejection::Reset => {
            fx.peer_open_stream(1, Method::Post, "/", false);
            let _conn = expect_conn(&mut fx);
            fx.peer_rst_stream(1, H2ErrorCode::Cancel);
            assert!(fx.tick().is_pending());
            setup.code = H2ErrorCode::StreamClosed;
        }
        ServerRejection::SelfDependent => {
            setup.priority = Some(PriorityInfo {
                exclusive: false,
                stream_dependency: 1,
                weight: 16,
            });
        }
        ServerRejection::Malformed => setup.prefix = METHOD_GET,
    }
    fx.next_outbound_frames();
    (fx, setup)
}

fn server_rejected_block_keeps_hpack_in_sync(rejection: ServerRejection, delivery: Delivery) {
    let (mut fx, mut setup) = server_with_rejection(rejection);
    let block = [RESTORE_TABLE_SIZE, setup.prefix, GET_REQUEST, INSERT_SYNC].concat();
    let mut occupant = setup.occupant.take();
    let frames = deliver_block(&mut fx, setup.id, &block, setup.priority, delivery, |_| {
        // An occupant leaving mid-block doesn't un-refuse a stream refused at its HEADERS frame.
        drop(occupant.take());
    });
    let context = format!("{rejection:?} {delivery:?}");
    assert_eq!(
        rst_on(&frames, setup.id),
        Some(setup.code),
        "{context}: {frames:?}"
    );
    assert_no_goaway(&frames);

    drop(occupant);
    assert!(fx.tick().is_pending());
    fx.next_outbound_frames();

    let next_id = setup.id + 2;
    peer_headers_fragment(
        &mut fx,
        next_id,
        &[GET_REQUEST, USE_SYNC].concat(),
        None,
        true,
    );
    let conn = expect_conn(&mut fx);
    assert_eq!(
        conn.request_headers().get_str("x-sync"),
        Some("probe"),
        "{context}"
    );
    assert_no_goaway(&fx.next_outbound_frames());
}

const ALL_DELIVERIES: [Delivery; 3] = [Delivery::Whole, Delivery::Split, Delivery::Interleaved];

#[test]
fn refused_stream_block_keeps_hpack_in_sync() {
    for delivery in ALL_DELIVERIES {
        server_rejected_block_keeps_hpack_in_sync(ServerRejection::Refused, delivery);
    }
}

#[test]
fn reset_stream_block_keeps_hpack_in_sync() {
    for delivery in ALL_DELIVERIES {
        server_rejected_block_keeps_hpack_in_sync(ServerRejection::Reset, delivery);
    }
}

#[test]
fn self_dependent_stream_block_keeps_hpack_in_sync() {
    for delivery in ALL_DELIVERIES {
        server_rejected_block_keeps_hpack_in_sync(ServerRejection::SelfDependent, delivery);
    }
}

#[test]
fn malformed_request_block_keeps_hpack_in_sync() {
    for delivery in ALL_DELIVERIES {
        server_rejected_block_keeps_hpack_in_sync(ServerRejection::Malformed, delivery);
    }
}

/// A rejected block is still decoded, so a compression error inside it still fails the
/// connection rather than being absorbed by the stream reset.
#[test]
fn compression_error_in_rejected_block_closes_connection() {
    for rejection in [
        ServerRejection::Refused,
        ServerRejection::Reset,
        ServerRejection::SelfDependent,
    ] {
        let (mut fx, setup) = server_with_rejection(rejection);
        let block = [GET_REQUEST, UNKNOWN_INDEX].concat();
        let frames = deliver_block(
            &mut fx,
            setup.id,
            &block,
            setup.priority,
            Delivery::Whole,
            |_| {},
        );
        assert!(
            frames.iter().any(|frame| matches!(
                frame,
                Frame::Goaway {
                    error_code: H2ErrorCode::CompressionError,
                    ..
                }
            )),
            "{rejection:?}: {frames:?}"
        );
    }
}

/// The application cancels a stream while its response block is still arriving. The block was sent
/// before the peer could know, so it's decoded and dropped without another reset.
#[test]
fn block_for_stream_canceled_mid_block_is_dropped_in_sync() {
    let mut fx = DriverFixture::new_client();
    fx.complete_handshake_client();
    let (canceled, transport) = open_get(&mut fx);
    let mut transport = Some(transport);
    let frames = deliver_block(
        &mut fx,
        canceled,
        &[STATUS_OK, INSERT_SYNC].concat(),
        None,
        Delivery::Interleaved,
        |_| drop(transport.take()),
    );
    let resets = frames
        .iter()
        .filter(
            |frame| matches!(frame, Frame::RstStream { stream_id, .. } if *stream_id == canceled),
        )
        .count();
    assert_eq!(resets, 1, "only the cancellation itself: {frames:?}");
    assert_no_goaway(&frames);

    let (next, _transport) = open_get(&mut fx);
    peer_headers_fragment(&mut fx, next, &[STATUS_OK, USE_SYNC].concat(), None, true);
    assert!(fx.tick().is_pending());
    assert_no_goaway(&fx.next_outbound_frames());
    assert_response_header(&fx, next, "x-sync", "probe");
}

fn assert_response_header(fx: &DriverFixture, id: u32, name: &'static str, value: &str) {
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let Poll::Ready(Ok(field_section)) =
        Pin::new(&mut fx.connection.response_headers(id)).poll(&mut cx)
    else {
        panic!("response headers on stream {id} not ready");
    };
    assert_eq!(field_section.headers().get_str(name), Some(value));
}

/// The server resets a stream its handler abandoned while the client's trailers were already on
/// the way. The block is decoded and dropped without another reset.
#[test]
fn late_block_on_locally_reset_server_stream_keeps_hpack_in_sync() {
    for delivery in [Delivery::Whole, Delivery::Split] {
        let mut fx = DriverFixture::new_server();
        fx.complete_handshake();
        fx.peer_open_stream(1, Method::Post, "/", false);
        drop(expect_conn(&mut fx));
        let _ = fx.tick();
        assert_rst(&fx.next_outbound_frames(), 1, H2ErrorCode::Cancel);

        let block = [RESTORE_TABLE_SIZE, INSERT_SYNC].concat();
        let frames = deliver_block(&mut fx, 1, &block, None, delivery, |_| {});
        assert_eq!(rst_on(&frames, 1), None, "{delivery:?}: {frames:?}");
        assert_no_goaway(&frames);

        peer_headers_fragment(&mut fx, 3, &[GET_REQUEST, USE_SYNC].concat(), None, true);
        let conn = expect_conn(&mut fx);
        assert_eq!(conn.request_headers().get_str("x-sync"), Some("probe"));
    }
}

#[test]
fn late_block_on_canceled_client_stream_keeps_hpack_in_sync() {
    for delivery in [Delivery::Whole, Delivery::Split] {
        let mut fx = DriverFixture::new_client();
        fx.complete_handshake_client();
        let (canceled, transport) = open_get(&mut fx);
        drop(transport);
        assert!(fx.tick().is_pending());
        fx.next_outbound_frames();

        let (next, _transport) = open_get(&mut fx);
        let block = [RESTORE_TABLE_SIZE, STATUS_OK, INSERT_SYNC].concat();
        let frames = deliver_block(&mut fx, canceled, &block, None, delivery, |_| {});
        assert_eq!(rst_on(&frames, canceled), None, "{delivery:?}: {frames:?}");
        assert_no_goaway(&frames);

        peer_headers_fragment(&mut fx, next, &[STATUS_OK, USE_SYNC].concat(), None, true);
        assert!(fx.tick().is_pending());
        assert_no_goaway(&fx.next_outbound_frames());
        assert_response_header(&fx, next, "x-sync", "probe");
    }
}
