//! Integration tests for `trillium-http`'s HTTP/2 implementation, speaking to hyper's `h2` crate
//! as a conformant peer over an in-memory duplex.
//!
//! Phase 1 coverage: preface + SETTINGS handshake (driven by hyper `h2`), PING round-trip, and
//! clean GOAWAY on swansong shutdown. Later phases extend this file with real request/response
//! cycles once `H2Connection` owns streams.

use async_compat::Compat;
use futures_lite::AsyncReadExt as _;
use h2::{Ping, client};
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex},
    sync::mpsc,
};
use trillium_http::{
    HttpContext,
    h2::{H2Connection, H2Transport},
};

/// RFC 9113 §3.4 client connection preface.
const CLIENT_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// `h2` frame-type bytes we care about in the raw tests.
const FRAME_TYPE_SETTINGS: u8 = 0x4;
const FRAME_TYPE_GOAWAY: u8 = 0x7;
const FLAG_ACK: u8 = 0x1;

fn spawn_server<T>(
    transport: T,
) -> (
    Arc<H2Connection>,
    mpsc::UnboundedReceiver<H2Transport>,
    tokio::task::JoinHandle<()>,
)
where
    T: futures_lite::io::AsyncRead + futures_lite::io::AsyncWrite + Unpin + Send + 'static,
{
    let _ = env_logger::try_init();
    let context = Arc::new(HttpContext::default());
    let conn = H2Connection::new(context);
    let conn_handle = conn.clone();
    let (tx, rx) = mpsc::unbounded_channel();
    let join = tokio::spawn(async move {
        let mut acceptor = conn.run(transport);
        loop {
            match acceptor.next().await {
                Ok(None) | Err(_) => break,
                Ok(Some(transport)) => {
                    // Hand the opened stream off to the test. If the receiver has been dropped,
                    // we silently discard (the test is no longer interested).
                    let _ = tx.send(transport);
                }
            }
        }
    });
    (conn_handle, rx, join)
}

/// Handshake + PING round-trip against hyper's `h2` client, then graceful shutdown.
///
/// Confirms the full phase-1 control path with a conformant peer:
/// - client preface is accepted,
/// - server SETTINGS is emitted in the right place,
/// - peer SETTINGS is ACKed,
/// - PING is echoed,
/// - shutdown produces a clean GOAWAY (the hyper `Connection` future resolves Ok).
#[tokio::test]
async fn hyper_h2_handshake_ping_and_shutdown() {
    let (client_io, server_io) = duplex(64 * 1024);
    let (conn, _streams, server_task) = spawn_server(Compat::new(server_io));

    let (_send_request, mut connection) = client::handshake(client_io)
        .await
        .expect("hyper h2 handshake failed");

    // PingPong has to be taken before the connection future is moved into spawn.
    let mut ping_pong = connection.ping_pong().expect("first ping_pong() is Some");

    let connection_task = tokio::spawn(connection);

    // The round-trip only resolves if the server both reads the PING frame and writes the ACK
    // back over the duplex — exercises the full read/dispatch/write loop.
    ping_pong
        .ping(Ping::opaque())
        .await
        .expect("PING not acked");

    conn.shut_down();

    connection_task
        .await
        .expect("client task panicked")
        .expect("client connection saw protocol error");
    server_task.await.expect("server task panicked");
}

/// Raw-byte probe of the GOAWAY wire format on graceful shutdown.
///
/// Bypasses hyper to assert the exact payload (last_stream_id=0, error_code=0 / NO_ERROR) rather
/// than just the Connection-future's Ok resolution.
#[tokio::test]
async fn shutdown_emits_goaway_with_no_error() {
    let (mut client_io, server_io) = duplex(64 * 1024);
    let (conn, _streams, _server_task) = spawn_server(Compat::new(server_io));

    client_io.write_all(CLIENT_PREFACE).await.unwrap();
    write_empty_settings(&mut client_io).await;

    // Drain the server's initial SETTINGS and any SETTINGS ACK before triggering shutdown, so the
    // GOAWAY is the next thing we read.
    let (header, _payload) = read_frame(&mut client_io).await;
    assert_eq!(header.frame_type, FRAME_TYPE_SETTINGS);
    assert_eq!(
        header.flags & FLAG_ACK,
        0,
        "first server frame must be non-ACK SETTINGS"
    );

    conn.shut_down();

    let goaway = loop {
        let (header, payload) = read_frame(&mut client_io).await;
        if header.frame_type == FRAME_TYPE_GOAWAY {
            break (header, payload);
        }
    };

    assert_eq!(goaway.0.stream_id, 0, "GOAWAY must be on stream 0");
    assert_eq!(
        goaway.1.len(),
        8,
        "GOAWAY payload: last_stream_id(4) + error_code(4)"
    );
    let last_stream_id =
        u32::from_be_bytes([goaway.1[0], goaway.1[1], goaway.1[2], goaway.1[3]]) & 0x7FFF_FFFF;
    let error_code = u32::from_be_bytes([goaway.1[4], goaway.1[5], goaway.1[6], goaway.1[7]]);
    assert_eq!(last_stream_id, 0);
    assert_eq!(error_code, 0, "graceful shutdown uses NO_ERROR");
}

/// A hand-crafted HEADERS frame for `GET https://example.com/hello` opens a stream end-to-end.
///
/// The block is HPACK-indexed entirely from the static table — no incremental indexing — to
/// keep the test independent of the encoder under test. Confirms preface → SETTINGS → HEADERS
/// decode → stream emit on the acceptor.
#[tokio::test]
async fn opens_stream_from_get_request() {
    let (mut client_io, server_io) = duplex(64 * 1024);
    let (conn, mut streams, server_task) = spawn_server(Compat::new(server_io));

    client_io.write_all(CLIENT_PREFACE).await.unwrap();
    write_empty_settings(&mut client_io).await;

    // Drain server SETTINGS so the wire is clean for any subsequent reads from the client side.
    // (We don't actually read here — just confirm the server didn't error early.)

    // HEADERS payload: index references against the static table.
    //   0x82 → :method GET (static index 2)
    //   0x87 → :scheme https (static index 7)
    //   0x44 → literal value, name index 4 (:path), then string "/hello"
    //   0x41 → literal value, name index 1 (:authority), then string "example.com"
    //
    // Note: 0x44 = 0b0100_0100 — Literal With Incremental Indexing, name index 4. We pick this
    // representation rather than 0x04 (without indexing) because either works on decode and the
    // dynamic table mutation is safe to take at face value (we have no follow-up references).
    // Strings are sent without Huffman to keep the bytes obvious.
    let mut block = vec![0x82, 0x87];
    // :path = "/hello"
    block.push(0x44);
    block.push(b"/hello".len() as u8);
    block.extend_from_slice(b"/hello");
    // :authority = "example.com"
    block.push(0x41);
    block.push(b"example.com".len() as u8);
    block.extend_from_slice(b"example.com");

    write_frame(
        &mut client_io,
        0x1,       // HEADERS
        0x4 | 0x1, // END_HEADERS | END_STREAM
        1,         // stream id
        &block,
    )
    .await;

    let opened = tokio::time::timeout(std::time::Duration::from_secs(2), streams.recv())
        .await
        .expect("acceptor did not emit a stream within 2s")
        .expect("acceptor closed before emitting a stream");

    assert_eq!(opened.stream_id(), 1);

    drop(opened);
    drop(client_io);
    conn.shut_down();
    server_task.await.expect("server task panicked");
}

/// A POST with a small body: HEADERS without END_STREAM, followed by one DATA frame with
/// END_STREAM. The handler reads the body off the H2Transport and asserts the bytes match.
#[tokio::test]
async fn handler_reads_request_body_from_data_frame() {
    let (mut client_io, server_io) = duplex(64 * 1024);
    let (conn, mut streams, server_task) = spawn_server(Compat::new(server_io));

    client_io.write_all(CLIENT_PREFACE).await.unwrap();
    write_empty_settings(&mut client_io).await;

    // HEADERS for `POST /upload` (no END_STREAM — body follows).
    let mut block = vec![0x83, 0x87]; // :method POST (3), :scheme https (7)
    block.push(0x44); // :path literal
    block.push(b"/upload".len() as u8);
    block.extend_from_slice(b"/upload");
    block.push(0x41); // :authority literal
    block.push(b"example.com".len() as u8);
    block.extend_from_slice(b"example.com");
    write_frame(
        &mut client_io,
        0x1, // HEADERS
        0x4, // END_HEADERS only — no END_STREAM
        1,   // stream id
        &block,
    )
    .await;

    // DATA frame with the body, END_STREAM set.
    let body = b"hello, body";
    write_frame(
        &mut client_io,
        0x0, // DATA
        0x1, // END_STREAM
        1,
        body,
    )
    .await;

    let mut transport = tokio::time::timeout(std::time::Duration::from_secs(2), streams.recv())
        .await
        .expect("acceptor did not emit a stream within 2s")
        .expect("acceptor closed before emitting a stream");

    // Drain the body via H2Transport's AsyncRead. The driver will route the DATA frame in the
    // background as we await.
    let mut got = Vec::new();
    transport.read_to_end(&mut got).await.expect("read body");
    assert_eq!(got.as_slice(), body);

    drop(transport);
    drop(client_io);
    conn.shut_down();
    server_task.await.expect("server task panicked");
}

/// Writes one HTTP/2 frame with the given type, flags, stream id, and payload.
async fn write_frame(
    client: &mut DuplexStream,
    frame_type: u8,
    flags: u8,
    stream_id: u32,
    payload: &[u8],
) {
    let len = payload.len() as u32;
    let mut hdr = [0u8; 9];
    hdr[0] = (len >> 16) as u8;
    hdr[1] = (len >> 8) as u8;
    hdr[2] = len as u8;
    hdr[3] = frame_type;
    hdr[4] = flags;
    hdr[5..9].copy_from_slice(&stream_id.to_be_bytes());
    client.write_all(&hdr).await.unwrap();
    if !payload.is_empty() {
        client.write_all(payload).await.unwrap();
    }
}

/// Writes a zero-length client SETTINGS frame. Enough to satisfy the server's handshake read.
async fn write_empty_settings(client: &mut DuplexStream) {
    let mut buf = [0u8; 9];
    // length = 0, type = SETTINGS, flags = 0, stream_id = 0
    buf[3] = FRAME_TYPE_SETTINGS;
    client.write_all(&buf).await.unwrap();
}

struct RawFrameHeader {
    length: u32,
    frame_type: u8,
    flags: u8,
    stream_id: u32,
}

async fn read_frame(client: &mut DuplexStream) -> (RawFrameHeader, Vec<u8>) {
    let mut hdr = [0u8; 9];
    client
        .read_exact(&mut hdr)
        .await
        .expect("frame header read");
    let header = RawFrameHeader {
        length: u32::from_be_bytes([0, hdr[0], hdr[1], hdr[2]]),
        frame_type: hdr[3],
        flags: hdr[4],
        stream_id: u32::from_be_bytes([hdr[5], hdr[6], hdr[7], hdr[8]]) & 0x7FFF_FFFF,
    };
    let mut payload = vec![0u8; header.length as usize];
    if !payload.is_empty() {
        client
            .read_exact(&mut payload)
            .await
            .expect("frame payload read");
    }
    (header, payload)
}
