//! RFC 8441 §3 extended-CONNECT gate: the client must NOT send a HEADERS frame carrying
//! `:protocol` until the peer has advertised `SETTINGS_ENABLE_CONNECT_PROTOCOL`.
//!
//! Drives a hand-rolled HTTP/2 "server" — a TCP socket reading and writing raw frames —
//! that completes the connection preface and SETTINGS handshake *without* enabling extended
//! CONNECT. The trillium-client then attempts a WebSocket-over-h2 upgrade. The test passes
//! only if the server *never* observes a HEADERS frame from the client; the client outcome
//! is asserted to be `ExtendedConnectUnsupported`.
//!
//! Failure mode this test catches: prior to the `peer_settings` gate, the
//! client sent `:method=CONNECT, :protocol=websocket` HEADERS *before* checking peer
//! capability. Without the fix, this test sees a HEADERS frame on the wire and surfaces it.

use async_net::{TcpListener, TcpStream};
use futures_lite::{AsyncReadExt as _, AsyncWriteExt as _, FutureExt as _};
use std::time::Duration;
use trillium_client::{Client, Version, websocket};
use trillium_testing::{TestResult, harness, test};

/// RFC 9113 §3.4 client connection preface.
const CLIENT_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

const FRAME_TYPE_HEADERS: u8 = 0x1;
const FRAME_TYPE_SETTINGS: u8 = 0x4;
const FLAG_ACK: u8 = 0x1;

async fn read_preface(stream: &mut TcpStream) {
    log::trace!("server: awaiting preface");
    let mut buf = [0u8; CLIENT_PREFACE.len()];
    stream.read_exact(&mut buf).await.expect("preface read");
    assert_eq!(&buf, CLIENT_PREFACE, "client preface mismatch");
    log::trace!("server: preface ok");
}

async fn read_frame(stream: &mut TcpStream) -> std::io::Result<(u8, u8, u32, Vec<u8>)> {
    let mut hdr = [0u8; 9];
    stream.read_exact(&mut hdr).await?;
    let length = u32::from_be_bytes([0, hdr[0], hdr[1], hdr[2]]);
    let frame_type = hdr[3];
    let flags = hdr[4];
    let stream_id = u32::from_be_bytes([hdr[5], hdr[6], hdr[7], hdr[8]]) & 0x7FFF_FFFF;
    let mut payload = vec![0u8; length as usize];
    if length > 0 {
        stream.read_exact(&mut payload).await?;
    }
    log::trace!(
        "server: read frame type=0x{frame_type:02x} flags=0x{flags:02x} stream_id={stream_id} \
         len={length}"
    );
    Ok((frame_type, flags, stream_id, payload))
}

async fn write_frame(
    stream: &mut TcpStream,
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
    stream.write_all(&hdr).await.expect("frame header write");
    if !payload.is_empty() {
        stream
            .write_all(payload)
            .await
            .expect("frame payload write");
    }
    log::trace!(
        "server: wrote frame type=0x{frame_type:02x} flags=0x{flags:02x} stream_id={stream_id} \
         len={len}"
    );
}

/// Accept one TCP connection, drive a minimal h2 handshake that does NOT advertise
/// `SETTINGS_ENABLE_CONNECT_PROTOCOL`, and read frames from the client until either a
/// HEADERS frame arrives (return immediately — spec violation observed) or the grace window
/// elapses (return whatever was collected).
async fn capture_client_frames(listener: TcpListener, grace: Duration) -> Vec<u8> {
    log::info!("server: awaiting accept on {:?}", listener.local_addr());
    let (mut stream, peer) = listener.accept().await.expect("accept");
    log::info!("server: accepted from {peer:?}");
    read_preface(&mut stream).await;

    // Send our (empty) SETTINGS — explicitly does not advertise enable_connect_protocol.
    log::info!("server: sending empty SETTINGS (no enable_connect_protocol)");
    write_frame(&mut stream, FRAME_TYPE_SETTINGS, 0, 0, &[]).await;

    let mut frame_types = Vec::new();
    let deadline = std::time::Instant::now() + grace;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            log::info!("server: grace window expired; collected frames: {frame_types:?}");
            return frame_types;
        }
        let read = async { Some(read_frame(&mut stream).await) };
        let timeout = async {
            async_io::Timer::after(remaining).await;
            None
        };
        match read.or(timeout).await {
            Some(Ok((frame_type, flags, _stream_id, _payload))) => {
                frame_types.push(frame_type);
                if frame_type == FRAME_TYPE_HEADERS {
                    log::warn!(
                        "server: client sent HEADERS — wire-level RFC 8441 §3 violation. \
                         Returning early."
                    );
                    return frame_types;
                }
                if frame_type == FRAME_TYPE_SETTINGS && flags & FLAG_ACK == 0 {
                    log::trace!("server: ACKing client SETTINGS");
                    write_frame(&mut stream, FRAME_TYPE_SETTINGS, FLAG_ACK, 0, &[]).await;
                }
            }
            Some(Err(e)) => {
                log::info!("server: read error {e:?}; returning collected frames: {frame_types:?}");
                return frame_types;
            }
            None => {
                log::info!("server: read timed out; collected frames: {frame_types:?}");
                return frame_types;
            }
        }
    }
}

/// One of two completing futures in the test's `or` race.
enum Outcome {
    /// Server returned first — its full set of frame types is the assertion target.
    Server(Vec<u8>),
    /// Client returned first — frame types are not yet final, but if we got `Err` we know no
    /// `:protocol` HEADERS was sent (the gate runs before any frame goes out).
    Client(Result<trillium_client::WebSocketConn, websocket::WebSocketUpgradeError>),
}

#[test(harness)]
async fn extended_connect_never_sends_protocol_pseudo_without_peer_setting() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();

    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    log::info!("test: bound listener on port {port}");

    let server = async {
        let frames = capture_client_frames(listener, Duration::from_millis(500)).await;
        log::info!("test: server branch finishing with frames {frames:?}");
        Outcome::Server(frames)
    };

    let client = async {
        let client = Client::new(trillium_smol::ClientConfig::default())
            .with_base(format!("http://127.0.0.1:{port}"));
        log::info!("test: client branch starting upgrade");
        let result = client
            .get("/")
            .with_http_version(Version::Http2)
            .into_websocket()
            .await;
        log::info!(
            "test: client branch finished with {}",
            match &result {
                Ok(_) => "Ok".to_string(),
                Err(e) => format!("Err({:?})", e.kind),
            }
        );
        Outcome::Client(result)
    };

    // Whole-test deadline so a misbehaving client that hangs forever still fails fast.
    let timeout = async {
        async_io::Timer::after(Duration::from_secs(5)).await;
        panic!("test deadline reached without either branch completing")
    };

    match server.or(client).or(timeout).await {
        Outcome::Server(frames) => {
            log::info!("test: server branch won");
            assert!(
                !frames.contains(&FRAME_TYPE_HEADERS),
                "client sent a HEADERS frame before the peer advertised \
                 SETTINGS_ENABLE_CONNECT_PROTOCOL — RFC 8441 §3 violation. Frames received: \
                 {frames:?}"
            );
            // Server completed cleanly without observing HEADERS, but the client's outcome is
            // still pending. Under the fix, the client either already returned Err
            // (race-won on its branch) or will return Err very soon now that SETTINGS has
            // arrived. The fact that we landed in this branch means we don't have a final
            // client outcome — that's fine: the wire-level invariant is the load-bearing
            // assertion, and the existing extended_connect_unsupported_when_server_lacks_setting
            // test in h2_websocket.rs covers the user-visible error path against a real
            // trillium server.
        }
        Outcome::Client(Ok(_)) => panic!("expected ExtendedConnectUnsupported, client returned Ok"),
        Outcome::Client(Err(err)) => {
            log::info!("test: client branch won with err {:?}", err.kind);
            assert!(
                matches!(err.kind, websocket::ErrorKind::ExtendedConnectUnsupported),
                "expected ExtendedConnectUnsupported, got {:?}",
                err.kind
            );
            // The client errored before any frame went out (the gate runs before
            // open_connect_stream), so by construction no `:protocol` HEADERS was sent.
        }
    }

    Ok(())
}
