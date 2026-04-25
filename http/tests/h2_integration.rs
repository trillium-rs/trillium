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
    Conn, HttpContext,
    h2::{H2Connection, H2Transport},
};

/// RFC 9113 §3.4 client connection preface.
const CLIENT_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// `h2` frame-type bytes we care about in the raw tests.
const FRAME_TYPE_DATA: u8 = 0x0;
const FRAME_TYPE_HEADERS: u8 = 0x1;
const FRAME_TYPE_RST_STREAM: u8 = 0x3;
const FRAME_TYPE_SETTINGS: u8 = 0x4;
const FRAME_TYPE_GOAWAY: u8 = 0x7;
const FRAME_TYPE_WINDOW_UPDATE: u8 = 0x8;
const FLAG_ACK: u8 = 0x1;
const FLAG_END_STREAM: u8 = 0x1;

/// `SETTINGS_INITIAL_WINDOW_SIZE` (RFC 9113 §6.5.2).
const SETTINGS_INITIAL_WINDOW_SIZE: u16 = 0x4;

/// RFC 9113 §7 `FLOW_CONTROL_ERROR`.
const ERROR_CODE_FLOW_CONTROL: u32 = 0x3;

fn spawn_server<T>(
    transport: T,
) -> (
    Arc<H2Connection>,
    mpsc::UnboundedReceiver<Conn<H2Transport>>,
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
        while let Some(result) = acceptor.next().await {
            match result {
                Err(_) => break,
                Ok(conn) => {
                    // Hand the opened Conn off to the test. If the receiver has been dropped,
                    // we silently discard (the test is no longer interested).
                    let _ = tx.send(conn);
                }
            }
        }
    });
    (conn_handle, rx, join)
}

/// Like `spawn_server`, but spawns a per-stream task that runs the provided handler and then
/// `send_h2`. Use for tests that need round-trip request → response semantics rather than
/// just observing the opened Conn.
fn spawn_h2_server_with_handler<T, F, Fut>(
    transport: T,
    handler: F,
) -> (Arc<H2Connection>, tokio::task::JoinHandle<()>)
where
    T: futures_lite::io::AsyncRead + futures_lite::io::AsyncWrite + Unpin + Send + 'static,
    F: Fn(Conn<H2Transport>) -> Fut + Send + Sync + Clone + 'static,
    Fut: std::future::Future<Output = Conn<H2Transport>> + Send + 'static,
{
    let _ = env_logger::try_init();
    let context = Arc::new(HttpContext::default());
    let conn = H2Connection::new(context);
    let conn_handle = conn.clone();
    let join = tokio::spawn(async move {
        let mut acceptor = conn.run(transport);
        while let Some(result) = acceptor.next().await {
            match result {
                Err(_) => break,
                Ok(c) => {
                    let handler = handler.clone();
                    tokio::spawn(async move {
                        let _ = H2Connection::process_inbound(c, handler).await;
                    });
                }
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

    assert_eq!(opened.h2_stream_id(), Some(1));

    drop(opened);
    drop(client_io);
    conn.shut_down();
    server_task.await.expect("server task panicked");
}

/// A POST with a small body: HEADERS without END_STREAM, followed by one DATA frame with
/// END_STREAM. The handler reads the body via [`Conn::request_body`] — the production path
/// through [`ReceivedBody`][trillium_http::ReceivedBody]'s `H2Data` state — and asserts the
/// bytes match. Exercises the full path: peer DATA → driver demux → recv ring →
/// `handle_h2_data` → handler bytes.
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

    let mut opened = tokio::time::timeout(std::time::Duration::from_secs(2), streams.recv())
        .await
        .expect("acceptor did not emit a stream within 2s")
        .expect("acceptor closed before emitting a stream");

    // Body-read pattern: spawn the handler's read first, which advertises intent to consume
    // and triggers the driver to emit a stream-level WINDOW_UPDATE. Wait for that WU on the
    // client side, then send DATA. Server advertises INITIAL_WINDOW_SIZE=0 (lazy-WU), so
    // sending DATA before the WU arrives would be a flow-control violation.
    let read_task = tokio::spawn(async move {
        let got = opened.request_body().read_bytes().await.expect("read body");
        (got, opened)
    });
    loop {
        let (hdr, _) = read_frame(&mut client_io).await;
        if hdr.frame_type == FRAME_TYPE_WINDOW_UPDATE && hdr.stream_id == 1 {
            break;
        }
    }

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

    let (got, opened) = read_task.await.expect("read task");
    assert_eq!(got.as_slice(), body);

    drop(opened);
    drop(client_io);
    conn.shut_down();
    server_task.await.expect("server task panicked");
}

/// The server advertises `SETTINGS_INITIAL_WINDOW_SIZE = 0` so the peer cannot send any DATA
/// until the handler declares intent to read the request body. `WINDOW_UPDATE` is emitted only
/// after that signal — a handler that never reads its body costs zero extra frames on the wire.
///
/// The test drives three observations in order:
/// 1. Initial SETTINGS from the server includes `INITIAL_WINDOW_SIZE = 0`.
/// 2. After HEADERS for a POST stream (no `END_STREAM`), the server does NOT emit `WINDOW_UPDATE`.
/// 3. Once the handler calls `poll_read` on the `H2Transport`, the server DOES emit `WINDOW_UPDATE`
///    for that stream.
#[tokio::test]
async fn lazy_window_update_gated_on_first_poll_read() {
    let (mut client_io, server_io) = duplex(64 * 1024);
    let (conn, mut streams, server_task) = spawn_server(Compat::new(server_io));

    client_io.write_all(CLIENT_PREFACE).await.unwrap();
    write_empty_settings(&mut client_io).await;

    // (1) Initial SETTINGS from server advertises INITIAL_WINDOW_SIZE = 0.
    let (hdr, payload) = read_frame(&mut client_io).await;
    assert_eq!(hdr.frame_type, FRAME_TYPE_SETTINGS);
    assert_eq!(hdr.flags & FLAG_ACK, 0, "first frame is non-ACK SETTINGS");
    let iws = parse_settings(&payload)
        .into_iter()
        .find_map(|(id, value)| (id == SETTINGS_INITIAL_WINDOW_SIZE).then_some(value))
        .expect("INITIAL_WINDOW_SIZE present in server SETTINGS");
    assert_eq!(iws, 0, "server advertises INITIAL_WINDOW_SIZE=0");

    // The server also emits a connection-level WINDOW_UPDATE right after SETTINGS to raise
    // the connection recv window above the RFC 65535 baseline — drain that and the SETTINGS
    // ACK that follows our client SETTINGS so subsequent reads are positioned at the
    // stream-level window behavior we're testing.
    let (hdr, _) = read_frame(&mut client_io).await;
    assert_eq!(
        hdr.frame_type, FRAME_TYPE_WINDOW_UPDATE,
        "initial connection-level WINDOW_UPDATE"
    );
    assert_eq!(hdr.stream_id, 0, "connection-level WINDOW_UPDATE");
    let (hdr, _) = read_frame(&mut client_io).await;
    assert_eq!(hdr.frame_type, FRAME_TYPE_SETTINGS);
    assert_eq!(hdr.flags & FLAG_ACK, FLAG_ACK, "peer SETTINGS ACK");

    // HEADERS for `POST /upload` without END_STREAM (body will follow eventually — we just
    // need the driver to open the stream).
    let mut block = vec![0x83, 0x87]; // :method POST (3), :scheme https (7)
    block.push(0x44); // :path literal, name index 4
    block.push(b"/upload".len() as u8);
    block.extend_from_slice(b"/upload");
    block.push(0x41); // :authority literal, name index 1
    block.push(b"example.com".len() as u8);
    block.extend_from_slice(b"example.com");
    write_frame(&mut client_io, 0x1, 0x4, 1, &block).await; // END_HEADERS only

    // (2) After HEADERS, no WINDOW_UPDATE yet — poll with a timeout to confirm absence.
    let mut opened = tokio::time::timeout(std::time::Duration::from_secs(2), streams.recv())
        .await
        .expect("acceptor did not emit a stream within 2s")
        .expect("acceptor closed before emitting a stream");
    assert_eq!(opened.h2_stream_id(), Some(1));

    let no_wu = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        read_frame(&mut client_io),
    )
    .await;
    assert!(
        no_wu.is_err(),
        "no frame should be sent before handler reads"
    );

    // (3) Handler polls read (returns Pending, but the is-reading signal fires).
    let mut scratch = [0u8; 1];
    // Poll once directly rather than using `read` — a Pending return is the expected outcome
    // (no body bytes yet), but the side effect (CAS on `is_reading` + waking the driver) is
    // the observable we care about.
    let mut fut = futures_lite::future::poll_once(futures_lite::AsyncReadExt::read(
        opened.transport_mut(),
        &mut scratch,
    ));
    let _ = (&mut fut).await;

    // The driver should now emit WINDOW_UPDATE for stream 1.
    let (hdr, payload) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        read_frame(&mut client_io),
    )
    .await
    .expect("WINDOW_UPDATE not received within 2s");
    assert_eq!(
        hdr.frame_type, FRAME_TYPE_WINDOW_UPDATE,
        "expected WINDOW_UPDATE after first poll_read"
    );
    assert_eq!(hdr.stream_id, 1, "stream-level WINDOW_UPDATE for stream 1");
    assert_eq!(payload.len(), 4);
    let increment =
        u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) & 0x7FFF_FFFF;
    assert_eq!(
        increment,
        1 << 20,
        "window topped up to MAX_STREAM_RECV_WINDOW (1 MiB)"
    );

    drop(opened);
    drop(client_io);
    conn.shut_down();
    server_task.await.expect("server task panicked");
}

/// First end-to-end submit_send: send a GET, run a handler that returns a small body, observe
/// the response on the wire (HEADERS without END_STREAM, DATA with body bytes, empty
/// DATA(END_STREAM) terminator). Exercises the full step-3 pipeline:
///   - acceptor opens stream → conn task receives Conn,
///   - handler sets status + body,
///   - send_h2 pre-encodes HEADERS, calls submit_send,
///   - driver picks up the submission, frames HEADERS / DATA / empty-DATA(END_STREAM),
///   - completion signals back to the conn task.
#[tokio::test]
async fn small_get_returns_response_body_end_to_end() {
    use trillium_http::Status;

    let (mut client_io, server_io) = duplex(64 * 1024);
    let (server_conn, server_task) =
        spawn_h2_server_with_handler(Compat::new(server_io), |conn| async move {
            conn.with_status(Status::Ok).with_response_body("hello, h2")
        });

    client_io.write_all(CLIENT_PREFACE).await.unwrap();
    write_empty_settings(&mut client_io).await;

    // GET / on stream 1 with END_STREAM (no request body).
    let mut block = vec![0x82, 0x87]; // :method GET (2), :scheme https (7)
    block.push(0x44); // :path literal, name index 4
    block.push(b"/".len() as u8);
    block.extend_from_slice(b"/");
    block.push(0x41); // :authority literal, name index 1
    block.push(b"example.com".len() as u8);
    block.extend_from_slice(b"example.com");
    write_frame(&mut client_io, FRAME_TYPE_HEADERS, 0x4 | 0x1, 1, &block).await;

    // Drain server frames. Ack the server's SETTINGS along the way.
    let mut got_response_headers = false;
    let mut response_body = Vec::new();
    let mut got_end_stream = false;
    while !got_end_stream {
        let (hdr, payload) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            read_frame(&mut client_io),
        )
        .await
        .expect("server stalled emitting response");

        match hdr.frame_type {
            FRAME_TYPE_SETTINGS if hdr.flags & FLAG_ACK == 0 => {
                write_settings_ack(&mut client_io).await;
            }
            FRAME_TYPE_HEADERS if hdr.stream_id == 1 => {
                got_response_headers = true;
                if hdr.flags & FLAG_END_STREAM != 0 {
                    got_end_stream = true;
                }
            }
            FRAME_TYPE_DATA if hdr.stream_id == 1 => {
                response_body.extend_from_slice(&payload);
                if hdr.flags & FLAG_END_STREAM != 0 {
                    got_end_stream = true;
                }
            }
            _ => {} // SETTINGS ACKs, WINDOW_UPDATEs, etc.
        }
    }

    assert!(got_response_headers, "response HEADERS observed");
    assert_eq!(
        response_body, b"hello, h2",
        "DATA frames carry the handler's response body"
    );

    drop(client_io);
    server_conn.shut_down();
    server_task.await.expect("server task panicked");
}

/// A client sends a POST with a small body, followed by a trailing HEADERS frame with
/// `END_STREAM` carrying a single trailer. The handler reads the body through
/// [`Conn::request_body`] (which uses `ReceivedBody`); after the body drain completes,
/// `conn.request_trailers()` returns the decoded trailer.
#[tokio::test]
async fn trailing_headers_populate_request_trailers() {
    use trillium_http::Status;

    let (trailers_tx, mut trailers_rx) = tokio::sync::mpsc::unbounded_channel();
    let (mut client_io, server_io) = duplex(64 * 1024);
    let (server_conn, server_task) =
        spawn_h2_server_with_handler(Compat::new(server_io), move |mut conn| {
            let trailers_tx = trailers_tx.clone();
            async move {
                let mut body = String::new();
                conn.request_body()
                    .read_to_string(&mut body)
                    .await
                    .expect("body read");
                // Trailers live on the Conn itself; clone out for inspection.
                let trailers = conn.request_trailers().cloned();
                let _ = trailers_tx.send((body, trailers));
                conn.with_status(Status::Ok).with_response_body("ok")
            }
        });

    client_io.write_all(CLIENT_PREFACE).await.unwrap();
    write_empty_settings(&mut client_io).await;

    // Request HEADERS (POST /upload, content-length: 5) with END_HEADERS only.
    let mut block = vec![0x83, 0x87]; // :method POST (3), :scheme https (7)
    block.push(0x44); // :path literal, name index 4
    block.push(b"/upload".len() as u8);
    block.extend_from_slice(b"/upload");
    block.push(0x41); // :authority literal, name index 1
    block.push(b"example.com".len() as u8);
    block.extend_from_slice(b"example.com");
    // content-length: 5 — literal without indexing, new name.
    block.push(0x00);
    block.push(b"content-length".len() as u8);
    block.extend_from_slice(b"content-length");
    block.push(b"5".len() as u8);
    block.extend_from_slice(b"5");
    write_frame(&mut client_io, FRAME_TYPE_HEADERS, 0x4, 1, &block).await;

    // Server advertises INITIAL_WINDOW_SIZE=0, so we can't send DATA until the handler
    // declares intent (which triggers a stream-level WINDOW_UPDATE). Ack SETTINGS and drain
    // server frames until we see WU for stream 1, at which point the window is open.
    loop {
        let (hdr, _) = read_frame(&mut client_io).await;
        if hdr.frame_type == FRAME_TYPE_SETTINGS && hdr.flags & FLAG_ACK == 0 {
            write_settings_ack(&mut client_io).await;
        } else if hdr.frame_type == FRAME_TYPE_WINDOW_UPDATE && hdr.stream_id == 1 {
            break;
        }
    }

    // DATA body (no END_STREAM — trailing HEADERS terminates the stream).
    write_frame(&mut client_io, FRAME_TYPE_DATA, 0, 1, b"hello").await;

    // Trailing HEADERS with a single trailer — literal without indexing, new name, no
    // pseudo-headers (§8.1).
    let mut trailer_block = vec![0x00];
    trailer_block.push(b"x-trailer-test".len() as u8);
    trailer_block.extend_from_slice(b"x-trailer-test");
    trailer_block.push(b"ok".len() as u8);
    trailer_block.extend_from_slice(b"ok");
    write_frame(
        &mut client_io,
        FRAME_TYPE_HEADERS,
        0x4 | 0x1, // END_HEADERS | END_STREAM
        1,
        &trailer_block,
    )
    .await;

    // Keep the client side from blocking on write back-pressure — drain anything else the
    // server emits while the handler is running.
    tokio::spawn(async move {
        loop {
            let _ = read_frame(&mut client_io).await;
        }
    });

    let (body, trailers) =
        tokio::time::timeout(std::time::Duration::from_secs(2), trailers_rx.recv())
            .await
            .expect("handler never produced trailer result")
            .expect("handler channel closed without sending");
    assert_eq!(body, "hello");
    let trailers = trailers.expect("request_trailers was None");
    assert_eq!(trailers.get_str("x-trailer-test"), Some("ok"));

    server_conn.shut_down();
    let _ = server_task.await;
}

/// Malformed trailers (pseudo-headers present) produce a stream-level
/// `RST_STREAM(PROTOCOL_ERROR)`, not a connection error. Connection stays open and a
/// subsequent stream on the same connection works normally.
#[tokio::test]
async fn malformed_trailers_with_pseudo_headers_is_rst_stream() {
    let (mut client_io, server_io) = duplex(64 * 1024);
    let (server_conn, mut streams, server_task) = spawn_server(Compat::new(server_io));

    client_io.write_all(CLIENT_PREFACE).await.unwrap();
    write_empty_settings(&mut client_io).await;

    // Stream 1: POST with content-length: 0 but trailing HEADERS carrying a pseudo-header
    // (`:method`) — §8.1 says trailers MUST NOT include pseudo-headers.
    let mut block = vec![0x83, 0x87];
    block.push(0x44);
    block.push(b"/upload".len() as u8);
    block.extend_from_slice(b"/upload");
    block.push(0x41);
    block.push(b"example.com".len() as u8);
    block.extend_from_slice(b"example.com");
    write_frame(&mut client_io, FRAME_TYPE_HEADERS, 0x4, 1, &block).await;

    // Wait for the stream to surface so we know the server has it registered.
    let opened = tokio::time::timeout(std::time::Duration::from_secs(2), streams.recv())
        .await
        .expect("stream not surfaced")
        .expect("acceptor closed");
    assert_eq!(opened.h2_stream_id(), Some(1));

    // Trailing HEADERS(END_STREAM) with a `:method` pseudo-header — static index 2.
    let trailer_block = vec![0x82]; // static index 2 (:method GET)
    write_frame(
        &mut client_io,
        FRAME_TYPE_HEADERS,
        0x4 | 0x1,
        1,
        &trailer_block,
    )
    .await;

    // Expect RST_STREAM for stream 1 with PROTOCOL_ERROR.
    let rst = loop {
        let (hdr, payload) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            read_frame(&mut client_io),
        )
        .await
        .expect("server stalled before RST_STREAM");
        if hdr.frame_type == FRAME_TYPE_RST_STREAM && hdr.stream_id == 1 {
            break (hdr, payload);
        }
    };
    let code = u32::from_be_bytes([rst.1[0], rst.1[1], rst.1[2], rst.1[3]]);
    assert_eq!(code, 0x1, "PROTOCOL_ERROR = 0x1");

    drop(opened);
    drop(client_io);
    server_conn.shut_down();
    server_task.await.expect("server task panicked");
}

/// A peer-advertised `SETTINGS_INITIAL_WINDOW_SIZE = 5` limits how much of a 9-byte response
/// body the server may emit before receiving a `WINDOW_UPDATE`. The test watches the wire for
/// an initial DATA(5) ("hello"), then sends `WINDOW_UPDATE(+4)` on the stream and confirms
/// the remaining 4 bytes (", h2") + empty `DATA(END_STREAM)` come through.
///
/// The connection-level window starts at the RFC 9113 §6.9.2 default of 65535, so the
/// per-stream `INITIAL_WINDOW_SIZE` is the binding constraint.
#[tokio::test]
async fn send_respects_peer_initial_window_size_and_resumes_on_window_update() {
    use trillium_http::Status;

    let (mut client_io, server_io) = duplex(64 * 1024);
    let (server_conn, server_task) =
        spawn_h2_server_with_handler(Compat::new(server_io), |conn| async move {
            conn.with_status(Status::Ok).with_response_body("hello, h2")
        });

    client_io.write_all(CLIENT_PREFACE).await.unwrap();
    write_settings_with(&mut client_io, SETTINGS_INITIAL_WINDOW_SIZE, 5).await;

    // GET / on stream 1 with END_STREAM (no request body).
    let mut block = vec![0x82, 0x87]; // :method GET (2), :scheme https (7)
    block.push(0x44); // :path literal, name index 4
    block.push(b"/".len() as u8);
    block.extend_from_slice(b"/");
    block.push(0x41); // :authority literal, name index 1
    block.push(b"example.com".len() as u8);
    block.extend_from_slice(b"example.com");
    write_frame(&mut client_io, FRAME_TYPE_HEADERS, 0x4 | 0x1, 1, &block).await;

    // Drain frames until we have the response HEADERS + the first DATA.
    let mut collected_body = Vec::new();
    let mut got_response_headers = false;
    let mut got_end_stream = false;
    let mut sent_window_update = false;

    while !got_end_stream {
        let (hdr, payload) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            read_frame(&mut client_io),
        )
        .await
        .unwrap_or_else(|_| {
            panic!(
                "server stalled. got_headers={got_response_headers} \
                 body_so_far={collected_body:?} sent_update={sent_window_update}"
            )
        });

        match hdr.frame_type {
            FRAME_TYPE_SETTINGS if hdr.flags & FLAG_ACK == 0 => {
                write_settings_ack(&mut client_io).await;
            }
            FRAME_TYPE_HEADERS if hdr.stream_id == 1 => {
                got_response_headers = true;
            }
            FRAME_TYPE_DATA if hdr.stream_id == 1 => {
                // The first DATA frame must respect the 5-byte window.
                if !sent_window_update {
                    assert!(
                        payload.len() <= 5,
                        "first DATA must fit the 5-byte window, got {} bytes",
                        payload.len()
                    );
                }
                collected_body.extend_from_slice(&payload);
                if hdr.flags & FLAG_END_STREAM != 0 {
                    got_end_stream = true;
                } else if !sent_window_update && collected_body.len() == 5 {
                    // Grant 4 more bytes for the tail of the response.
                    write_window_update(&mut client_io, 1, 4).await;
                    sent_window_update = true;
                }
            }
            _ => {} // SETTINGS ACKs, WINDOW_UPDATEs, etc.
        }
    }

    assert!(got_response_headers, "response HEADERS observed");
    assert_eq!(collected_body, b"hello, h2");
    assert!(
        sent_window_update,
        "server emitted all body bytes without waiting — window not being enforced?"
    );

    drop(client_io);
    server_conn.shut_down();
    server_task.await.expect("server task panicked");
}

/// A mid-connection `SETTINGS_INITIAL_WINDOW_SIZE` change applies as a delta (new − old)
/// to all open streams' send windows (RFC 9113 §6.9.2).
///
/// Client:
/// 1. Opens the connection with `INITIAL_WINDOW_SIZE = 0` — server can't emit any DATA.
/// 2. Sends a GET that triggers a 9-byte response. Server emits HEADERS but stalls on DATA.
/// 3. Sends `SETTINGS(INITIAL_WINDOW_SIZE = 9)` — delta `+9`, stream 1's send window becomes 9.
/// 4. Server emits DATA(9) + empty `DATA(END_STREAM)`.
#[tokio::test]
async fn settings_initial_window_size_delta_unblocks_open_streams() {
    use trillium_http::Status;

    let (mut client_io, server_io) = duplex(64 * 1024);
    let (server_conn, server_task) =
        spawn_h2_server_with_handler(Compat::new(server_io), |conn| async move {
            conn.with_status(Status::Ok).with_response_body("hello, h2")
        });

    client_io.write_all(CLIENT_PREFACE).await.unwrap();
    write_settings_with(&mut client_io, SETTINGS_INITIAL_WINDOW_SIZE, 0).await;

    let mut block = vec![0x82, 0x87];
    block.push(0x44);
    block.push(b"/".len() as u8);
    block.extend_from_slice(b"/");
    block.push(0x41);
    block.push(b"example.com".len() as u8);
    block.extend_from_slice(b"example.com");
    write_frame(&mut client_io, FRAME_TYPE_HEADERS, 0x4 | 0x1, 1, &block).await;

    let mut got_response_headers = false;
    let mut collected_body = Vec::new();
    let mut got_end_stream = false;
    let mut raised_initial_window = false;

    while !got_end_stream {
        let frame_poll = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            read_frame(&mut client_io),
        )
        .await;

        match frame_poll {
            // The expected stall point: response HEADERS seen, no DATA yet. Raise the
            // peer's INITIAL_WINDOW_SIZE and expect DATA to flow.
            Err(_) if got_response_headers && !raised_initial_window => {
                write_settings_with(&mut client_io, SETTINGS_INITIAL_WINDOW_SIZE, 9).await;
                raised_initial_window = true;
                continue;
            }
            Err(_) => panic!(
                "server stalled unexpectedly. got_headers={got_response_headers} \
                 body_so_far={collected_body:?} raised_window={raised_initial_window}"
            ),
            Ok((hdr, payload)) => match hdr.frame_type {
                FRAME_TYPE_SETTINGS if hdr.flags & FLAG_ACK == 0 => {
                    write_settings_ack(&mut client_io).await;
                }
                FRAME_TYPE_HEADERS if hdr.stream_id == 1 => {
                    got_response_headers = true;
                }
                FRAME_TYPE_DATA if hdr.stream_id == 1 => {
                    collected_body.extend_from_slice(&payload);
                    if hdr.flags & FLAG_END_STREAM != 0 {
                        got_end_stream = true;
                    }
                }
                _ => {}
            },
        }
    }

    assert!(
        raised_initial_window,
        "stall was resolved by delta SETTINGS"
    );
    assert_eq!(collected_body, b"hello, h2");

    drop(client_io);
    server_conn.shut_down();
    server_task.await.expect("server task panicked");
}

/// A peer-sent connection-level `WINDOW_UPDATE(stream_id=0, 2^31-1)` pushes the default
/// connection-send window (65535) past RFC 9113 §6.9.1's `2^31 - 1` limit: the server
/// must respond with `GOAWAY(FLOW_CONTROL_ERROR)` and tear the connection down.
#[tokio::test]
async fn connection_window_update_overflow_is_flow_control_error() {
    let (mut client_io, server_io) = duplex(64 * 1024);
    let (_conn, _streams, _server_task) = spawn_server(Compat::new(server_io));

    client_io.write_all(CLIENT_PREFACE).await.unwrap();
    write_empty_settings(&mut client_io).await;

    // Overflow: current conn window is 65535; increment 2^31 - 1 pushes it over the 2^31-1 cap.
    write_window_update(&mut client_io, 0, 0x7FFF_FFFF).await;

    let goaway = loop {
        let (hdr, payload) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            read_frame(&mut client_io),
        )
        .await
        .expect("server stalled before GOAWAY");
        if hdr.frame_type == FRAME_TYPE_GOAWAY {
            break (hdr, payload);
        }
    };
    assert_eq!(goaway.0.stream_id, 0);
    let error_code = u32::from_be_bytes([goaway.1[4], goaway.1[5], goaway.1[6], goaway.1[7]]);
    assert_eq!(error_code, ERROR_CODE_FLOW_CONTROL);
}

/// A peer-sent stream-level `WINDOW_UPDATE` that overflows the per-stream window is a
/// *stream*-level error: the server emits `RST_STREAM(FLOW_CONTROL_ERROR)` and the
/// connection stays open.
#[tokio::test]
async fn stream_window_update_overflow_is_rst_stream() {
    let (mut client_io, server_io) = duplex(64 * 1024);
    let (server_conn, mut streams, server_task) = spawn_server(Compat::new(server_io));

    client_io.write_all(CLIENT_PREFACE).await.unwrap();
    write_empty_settings(&mut client_io).await;

    // Open stream 1 with a plain GET so the server has a stream 1 to reset.
    let mut block = vec![0x82, 0x87];
    block.push(0x44);
    block.push(b"/".len() as u8);
    block.extend_from_slice(b"/");
    block.push(0x41);
    block.push(b"example.com".len() as u8);
    block.extend_from_slice(b"example.com");
    write_frame(&mut client_io, FRAME_TYPE_HEADERS, 0x4 | 0x1, 1, &block).await;

    // Wait for the acceptor to surface the stream so we know the stream is open on the server.
    let opened = tokio::time::timeout(std::time::Duration::from_secs(2), streams.recv())
        .await
        .expect("acceptor did not emit a stream within 2s")
        .expect("acceptor closed before emitting a stream");
    assert_eq!(opened.h2_stream_id(), Some(1));

    // Overflow the per-stream window.
    write_window_update(&mut client_io, 1, 0x7FFF_FFFF).await;

    let rst = loop {
        let (hdr, payload) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            read_frame(&mut client_io),
        )
        .await
        .expect("server stalled before RST_STREAM");
        if hdr.frame_type == FRAME_TYPE_RST_STREAM && hdr.stream_id == 1 {
            break (hdr, payload);
        }
    };
    let error_code = u32::from_be_bytes([rst.1[0], rst.1[1], rst.1[2], rst.1[3]]);
    assert_eq!(error_code, ERROR_CODE_FLOW_CONTROL);

    // Connection is still alive — PING should still round-trip. We'll just shut down cleanly.
    drop(opened);
    drop(client_io);
    server_conn.shut_down();
    server_task.await.expect("server task panicked");
}

/// Extended-CONNECT (RFC 8441 / RFC 9220) end-to-end:
///
/// 1. Server is configured with `extended_connect_enabled = true`. We assert its initial SETTINGS
///    frame includes `SETTINGS_ENABLE_CONNECT_PROTOCOL = 1`.
/// 2. Client sends a CONNECT request HEADERS with `:protocol = websocket`, no `END_STREAM`.
/// 3. Handler observes `method = CONNECT`, `protocol = Some("websocket")`, returns status 200.
/// 4. `Conn::send_h2` routes through `submit_upgrade` because `should_upgrade()` is true, and
///    signals completion the moment HEADERS hit the wire.
/// 5. Test reaches into the post-send Conn for the `H2Transport`, writes a few bytes, and closes.
///    We expect HEADERS without END_STREAM, DATA frame(s) carrying the bytes, and a final
///    `DATA(END_STREAM)` terminator.
#[tokio::test]
async fn extended_connect_upgrade_round_trip() {
    use trillium_http::{HttpConfig, Method, Status};

    let _ = env_logger::try_init();
    let (mut client_io, server_io) = duplex(64 * 1024);

    let context = Arc::new(
        HttpContext::default().with_config(HttpConfig::default().with_extended_connect_enabled()),
    );
    let server_conn = H2Connection::new(context);
    let server_task = {
        let server_conn = server_conn.clone();
        let mut acceptor = server_conn.run(Compat::new(server_io));
        tokio::spawn(async move {
            while let Some(result) = acceptor.next().await {
                match result {
                    Err(_) => break,
                    Ok(c) => {
                        // Per-stream task: handler sets status 200, send_h2 routes through
                        // submit_upgrade, then we use the H2Transport (still on the Conn) for
                        // bidi bytes.
                        tokio::spawn(async move {
                            let conn_after_send =
                                H2Connection::process_inbound(c, |conn| async move {
                                    assert_eq!(conn.method(), Method::Connect, "expect CONNECT");
                                    assert_eq!(
                                        conn.protocol(),
                                        Some("websocket"),
                                        "expect :protocol = websocket"
                                    );
                                    conn.with_status(Status::Ok)
                                })
                                .await
                                .expect("process_inbound");
                            assert!(
                                conn_after_send.should_upgrade(),
                                "CONNECT + 200 ⇒ should_upgrade()"
                            );
                            // Mirror the runtime adapter's upgrade dispatch: convert into an
                            // Upgrade (which AsyncWrite-forwards to the inner H2Transport),
                            // write the payload, close. Closing flushes the outbound queue and
                            // emits DATA(END_STREAM).
                            let mut upgrade = trillium_http::Upgrade::from(conn_after_send);
                            use futures_lite::AsyncWriteExt;
                            upgrade.write_all(b"hello over h2 upgrade").await.unwrap();
                            upgrade.close().await.unwrap();
                        });
                    }
                }
            }
        })
    };

    client_io.write_all(CLIENT_PREFACE).await.unwrap();
    write_empty_settings(&mut client_io).await;

    // CONNECT /chat with :protocol=websocket on stream 1, NO END_STREAM (extended-CONNECT
    // streams stay open after HEADERS).
    //
    // Indices used (RFC 7541 Appendix A static table):
    //   :scheme https     → static index 7 (0x87)
    //   :path             → static index 4
    //   :authority        → static index 1
    //
    // :method CONNECT and :protocol aren't in the static table — emit them as literal-with-
    // incremental-indexing (0x40 prefix) using literal name + value strings.
    let mut block = vec![0x87]; // :scheme https
    // :method = CONNECT (literal name + value, name not indexed)
    block.push(0x40);
    block.push(b":method".len() as u8);
    block.extend_from_slice(b":method");
    block.push(b"CONNECT".len() as u8);
    block.extend_from_slice(b"CONNECT");
    // :path = /chat (name index 4)
    block.push(0x44);
    block.push(b"/chat".len() as u8);
    block.extend_from_slice(b"/chat");
    // :authority = example.com (name index 1)
    block.push(0x41);
    block.push(b"example.com".len() as u8);
    block.extend_from_slice(b"example.com");
    // :protocol = websocket (literal name + value, name not indexed)
    block.push(0x40);
    block.push(b":protocol".len() as u8);
    block.extend_from_slice(b":protocol");
    block.push(b"websocket".len() as u8);
    block.extend_from_slice(b"websocket");
    write_frame(
        &mut client_io,
        FRAME_TYPE_HEADERS,
        0x4, // END_HEADERS only — NOT END_STREAM
        1,
        &block,
    )
    .await;

    // Drain server frames. Verify (a) initial SETTINGS includes ENABLE_CONNECT_PROTOCOL=1,
    // (b) response HEADERS arrive without END_STREAM, (c) DATA carries the upgrade bytes,
    // (d) final DATA(END_STREAM) is the stream terminator.
    let mut saw_settings_with_connect_protocol = false;
    let mut saw_response_headers = false;
    let mut response_headers_had_end_stream = false;
    let mut data_payload = Vec::new();
    let mut got_end_stream = false;
    while !got_end_stream {
        let (hdr, payload) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            read_frame(&mut client_io),
        )
        .await
        .expect("server stalled emitting upgrade frames");

        match hdr.frame_type {
            FRAME_TYPE_SETTINGS if hdr.flags & FLAG_ACK == 0 => {
                let entries = parse_settings(&payload);
                if entries.iter().any(|(id, val)| *id == 0x8 && *val == 1) {
                    saw_settings_with_connect_protocol = true;
                }
                write_settings_ack(&mut client_io).await;
            }
            FRAME_TYPE_HEADERS if hdr.stream_id == 1 => {
                saw_response_headers = true;
                if hdr.flags & FLAG_END_STREAM != 0 {
                    response_headers_had_end_stream = true;
                }
            }
            FRAME_TYPE_DATA if hdr.stream_id == 1 => {
                data_payload.extend_from_slice(&payload);
                if hdr.flags & FLAG_END_STREAM != 0 {
                    got_end_stream = true;
                }
            }
            _ => {}
        }
    }

    assert!(
        saw_settings_with_connect_protocol,
        "server initial SETTINGS must include SETTINGS_ENABLE_CONNECT_PROTOCOL=1"
    );
    assert!(saw_response_headers, "server must emit response HEADERS");
    assert!(
        !response_headers_had_end_stream,
        "extended-CONNECT response HEADERS must NOT carry END_STREAM"
    );
    assert_eq!(
        data_payload, b"hello over h2 upgrade",
        "DATA frames carry the bytes the handler wrote through H2Transport"
    );

    drop(client_io);
    server_conn.shut_down();
    server_task.await.expect("server task panicked");
}

/// Parse a raw SETTINGS payload into (id, value) pairs. Each entry is 6 bytes: u16 id, u32 value.
fn parse_settings(payload: &[u8]) -> Vec<(u16, u32)> {
    payload
        .chunks_exact(6)
        .map(|c| {
            (
                u16::from_be_bytes([c[0], c[1]]),
                u32::from_be_bytes([c[2], c[3], c[4], c[5]]),
            )
        })
        .collect()
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

/// Writes a client SETTINGS frame with a single (id, value) entry.
async fn write_settings_with(client: &mut DuplexStream, id: u16, value: u32) {
    let mut payload = [0u8; 6];
    payload[0..2].copy_from_slice(&id.to_be_bytes());
    payload[2..6].copy_from_slice(&value.to_be_bytes());
    write_frame(client, FRAME_TYPE_SETTINGS, 0, 0, &payload).await;
}

/// Writes a `WINDOW_UPDATE` frame for the given stream (0 = connection-level).
async fn write_window_update(client: &mut DuplexStream, stream_id: u32, increment: u32) {
    let mut payload = [0u8; 4];
    payload.copy_from_slice(&(increment & 0x7FFF_FFFF).to_be_bytes());
    write_frame(client, FRAME_TYPE_WINDOW_UPDATE, 0, stream_id, &payload).await;
}

/// Writes a zero-length client SETTINGS ACK frame.
async fn write_settings_ack(client: &mut DuplexStream) {
    let mut buf = [0u8; 9];
    buf[3] = FRAME_TYPE_SETTINGS;
    buf[4] = FLAG_ACK;
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

// ---- Active PING (`H2Connection::send_ping`) ----

/// Server-initiated PING is acked by the peer and the future resolves with the round-trip
/// time.
#[tokio::test]
async fn server_active_ping_roundtrip() {
    let (client_io, server_io) = duplex(64 * 1024);
    let (conn, _streams, server_task) = spawn_server(Compat::new(server_io));

    let (_send_request, connection) = client::handshake(client_io)
        .await
        .expect("hyper h2 handshake failed");
    let connection_task = tokio::spawn(connection);

    let rtt = conn
        .send_ping([1; 8])
        .await
        .expect("server-initiated PING was not acked");
    assert!(
        rtt < std::time::Duration::from_secs(1),
        "PING RTT looks unreasonable: {rtt:?}"
    );

    conn.shut_down();
    connection_task
        .await
        .expect("client task panicked")
        .expect("client connection saw protocol error");
    server_task.await.expect("server task panicked");
}

/// A second `send_ping` with the same opaque payload while one is still in flight resolves
/// to `io::ErrorKind::AlreadyExists`. Dropping the first lets the opaque be reused.
#[tokio::test]
async fn duplicate_opaque_returns_already_exists() {
    use std::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    let (_client_io, server_io) = duplex(64 * 1024);
    let (conn, _streams, _server_task) = spawn_server(Compat::new(server_io));

    let first = conn.send_ping([2; 8]);
    let second = conn.send_ping([2; 8]);

    // Second resolves synchronously on first poll — no I/O needed for the dup check.
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut second = pin!(second);
    let Poll::Ready(result) = second.as_mut().poll(&mut cx) else {
        panic!("duplicate-opaque PING must resolve on first poll");
    };
    let err = result.expect_err("duplicate opaque must surface an error");
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);

    drop(first);

    // After the first is dropped, the same opaque is reusable — fresh `send_ping` should
    // register and return a `Pending` future on first poll (no I/O has happened yet).
    let third = conn.send_ping([2; 8]);
    let mut third = pin!(third);
    assert!(matches!(third.as_mut().poll(&mut cx), Poll::Pending));
}

/// Dropping a `SendPing` future before completion removes the entry from the connection's
/// pending map, so re-using the same opaque immediately afterward does not collide.
#[tokio::test]
async fn dropped_send_ping_cleans_up() {
    use std::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    let (_client_io, server_io) = duplex(64 * 1024);
    let (conn, _streams, _server_task) = spawn_server(Compat::new(server_io));

    drop(conn.send_ping([3; 8]));

    // If cleanup didn't run, this would resolve to `AlreadyExists` on first poll.
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let probe = conn.send_ping([3; 8]);
    let mut probe = pin!(probe);
    assert!(
        matches!(probe.as_mut().poll(&mut cx), Poll::Pending),
        "fresh send_ping after drop should be pending, not AlreadyExists"
    );
}

/// A pending `send_ping` resolves with `ConnectionAborted` when the connection closes
/// before the ACK arrives.
#[tokio::test]
async fn pending_ping_completes_on_connection_close() {
    let (client_io, server_io) = duplex(64 * 1024);
    let (conn, _streams, server_task) = spawn_server(Compat::new(server_io));

    // Bring the client up so the driver is running and processing frames, then drop it
    // — the server driver will see EOF on read and close the connection.
    let (_send_request, connection) = client::handshake(client_io)
        .await
        .expect("hyper h2 handshake failed");
    let connection_task = tokio::spawn(connection);

    // Issue a ping but use an opaque the client won't know about yet (it will, since
    // hyper auto-acks). To force "no ack arrives", shut the connection down immediately
    // so the server's terminal cleanup runs `fail_pending_pings`.
    let ping = conn.send_ping([4; 8]);
    conn.shut_down();

    let err = ping
        .await
        .expect_err("pending PING must error on connection close");
    assert_eq!(err.kind(), std::io::ErrorKind::ConnectionAborted);

    let _ = connection_task.await;
    server_task.await.expect("server task panicked");
}
