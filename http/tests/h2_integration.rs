//! Integration tests for `trillium-http`'s HTTP/2 implementation, speaking to hyper's `h2` crate
//! as a conformant peer over an in-memory duplex.
//!
//! Phase 1 coverage: preface + SETTINGS handshake (driven by hyper `h2`), PING round-trip, and
//! clean GOAWAY on swansong shutdown. Later phases extend this file with real request/response
//! cycles once `H2Connection` owns streams.

use async_compat::Compat;
use h2::{Ping, client};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};
use trillium_http::{HttpContext, h2::H2Connection};

/// RFC 9113 §3.4 client connection preface.
const CLIENT_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// `h2` frame-type bytes we care about in the raw tests.
const FRAME_TYPE_SETTINGS: u8 = 0x4;
const FRAME_TYPE_GOAWAY: u8 = 0x7;
const FLAG_ACK: u8 = 0x1;

fn spawn_server<T>(transport: T) -> (Arc<H2Connection>, tokio::task::JoinHandle<()>)
where
    T: futures_lite::io::AsyncRead + futures_lite::io::AsyncWrite + Unpin + Send + 'static,
{
    let _ = env_logger::try_init();
    let context = Arc::new(HttpContext::default());
    let conn = H2Connection::new(context);
    let conn_handle = conn.clone();
    let join = tokio::spawn(async move {
        let mut acceptor = conn.run(transport);
        // Phase-3 placeholder: no streams are emitted yet; the first call to next() drains the
        // connection and returns Ok(None) on shutdown. Errors here are expected on tests that
        // drop their client half early.
        loop {
            match acceptor.next().await {
                Ok(None) | Err(_) => break,
                Ok(Some(_transport)) => unreachable!("streams not yet implemented"),
            }
        }
    });
    (conn_handle, join)
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
    let (conn, server_task) = spawn_server(Compat::new(server_io));

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
    let (conn, _server_task) = spawn_server(Compat::new(server_io));

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
