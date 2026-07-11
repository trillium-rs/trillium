//! End-to-end tests for `Upgrade` across the protocol matrix.
//!
//! Each test spins up a per-protocol fixture from `tests/common/`, arms an upgrade on
//! the server, performs framing-aware reads/writes through `Upgrade`, and asserts
//! the wire round-trip against a real `trillium-client`.

mod common;

use common::{h1::H1Server, h2c::H2cServer, h3::H3Server};
use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, io::Cursor};
use std::{io, sync::Arc};
use test_harness::test;
use trillium_client::{Client, ConnExt, Version};
use trillium_http::{Body, Conn, Headers, Upgrade};
use trillium_quinn::ClientQuicConfig;
use trillium_rustls::{RustlsConfig, rustls};
use trillium_testing::{TestResult, harness};

/// Read bytes from `reader` until `\n` (inclusive) and return the line as a `String`.
/// Returns `Ok(None)` on clean EOF before any bytes are read.
async fn read_line_async<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = reader.read(&mut byte).await?;
        if n == 0 {
            return if buf.is_empty() {
                Ok(None)
            } else {
                Ok(Some(String::from_utf8(buf).unwrap()))
            };
        }
        buf.push(byte[0]);
        if byte[0] == b'\n' {
            return Ok(Some(String::from_utf8(buf).unwrap()));
        }
    }
}

/// Server-side ping-pong: read N lines (each terminated by `\n`) and echo each one back
/// with a `pong: ` prefix.
async fn server_pong<T>(upgrade: &mut Upgrade<T>, rounds: usize) -> io::Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    for _ in 0..rounds {
        let line = read_line_async(upgrade)
            .await?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "EOF mid-round"))?;
        let response = format!("pong: {line}");
        upgrade.write_all(response.as_bytes()).await?;
    }
    Ok(())
}

/// Echo `pong: <line>` for every line the peer sends until it half-closes, then close
/// this side. The EOF-driven loop (rather than a fixed round count) is what lets the h1
/// ping-pong tear down to a clean FIN; see `h1_ping_pong_bidi`.
async fn server_pong_to_eof<T>(mut upgrade: Upgrade<T>) -> io::Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    while let Some(line) = read_line_async(&mut upgrade).await? {
        let response = format!("pong: {line}");
        upgrade.write_all(response.as_bytes()).await?;
    }
    upgrade.close().await
}

/// Client-side ping-pong: for each round, write `ping: <round>\n` and assert the server
/// echoes back `pong: ping: <round>\n`.
async fn client_ping<T>(upgrade: &mut Upgrade<T>, rounds: usize) -> io::Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    for round in 0..rounds {
        let ping = format!("ping: {round}\n");
        upgrade.write_all(ping.as_bytes()).await?;
        let pong = read_line_async(upgrade)
            .await?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "no response"))?;
        assert_eq!(pong, format!("pong: ping: {round}\n"));
    }
    Ok(())
}

fn h3_client(server: &H3Server) -> Client {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.add(server.cert_der().clone()).unwrap();
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let tls = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    Client::new_with_quic(
        RustlsConfig::new(tls.clone(), trillium_smol::ClientConfig::default()),
        ClientQuicConfig::from_rustls_client_config(tls),
    )
}

#[test(harness)]
async fn h1_outbound_framing_and_trailers() -> TestResult {
    let server = H1Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        |mut upgrade: Upgrade<_>| async move {
            upgrade.write_all(b"hello framed").await.unwrap();
            let mut trailers = Headers::new();
            trailers.insert("x-result", "ok");
            upgrade.send_trailers(trailers).await.unwrap();
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let mut conn = client.get(server.base_url()).await?;

    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.response_body().read_string().await?, "hello framed");
    assert_eq!(
        conn.response_trailers().and_then(|t| t.get_str("x-result")),
        Some("ok"),
    );

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h3_outbound_framing_and_trailers() -> TestResult {
    let server = H3Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        |mut upgrade: Upgrade<_>| async move {
            upgrade.write_all(b"hello framed").await.unwrap();
            let mut trailers = Headers::new();
            trailers.insert("x-result", "ok");
            upgrade.send_trailers(trailers).await.unwrap();
        },
    )
    .await;

    let client = h3_client(&server);
    let mut conn = client
        .get(server.base_url())
        .with_http_version(Version::Http3)
        .await?;

    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http3);
    assert_eq!(conn.response_body().read_string().await?, "hello framed");
    assert_eq!(
        conn.response_trailers().and_then(|t| t.get_str("x-result")),
        Some("ok"),
    );

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h2c_outbound_framing_and_trailers() -> TestResult {
    let server = H2cServer::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        |mut upgrade: Upgrade<_>| async move {
            upgrade.write_all(b"hello framed").await.unwrap();
            let mut trailers = Headers::new();
            trailers.insert("x-result", "ok");
            upgrade.send_trailers(trailers).await.unwrap();
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let mut conn = client
        .get(server.base_url())
        .with_http_version(Version::Http2)
        .await?;

    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http2);
    assert_eq!(conn.response_body().read_string().await?, "hello framed");
    assert_eq!(
        conn.response_trailers().and_then(|t| t.get_str("x-result")),
        Some("ok"),
    );

    server.shut_down().await;
    Ok(())
}

const PING_ROUNDS: usize = 5;

// Unlike the h2c/h3 ping-pong tests, h1 tears down explicitly. An h1 upgrade rides a
// bare TcpStream; dropping it is an abortive close that never writes the chunked
// terminator, and a TCP stack that discards buffered data on RST (Windows) drops the
// peer's last frame along with it. h2 and h3 schedule a graceful close on drop, so their
// ping-pong tests can finish by just dropping. Here the client half-closes and the
// server reads to EOF before closing, draining both directions to a clean FIN.
#[test(harness)]
async fn h1_ping_pong_bidi() -> TestResult {
    let server = H1Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        |upgrade: Upgrade<_>| async move {
            server_pong_to_eof(upgrade).await.unwrap();
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client.post(server.base_url()).upgrade().await?;
    assert_eq!(conn.status().unwrap(), 200);
    let mut upgrade = Upgrade::from(conn);
    client_ping(&mut upgrade, PING_ROUNDS).await?;

    upgrade.close().await?;
    let mut drained = Vec::new();
    upgrade.read_to_end(&mut drained).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h2c_ping_pong_bidi() -> TestResult {
    let server = H2cServer::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        |mut upgrade: Upgrade<_>| async move {
            server_pong(&mut upgrade, PING_ROUNDS).await.unwrap();
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client
        .post(server.base_url())
        .with_http_version(Version::Http2)
        .upgrade()
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http2);
    let mut upgrade = Upgrade::from(conn);
    client_ping(&mut upgrade, PING_ROUNDS).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h3_ping_pong_bidi() -> TestResult {
    let server = H3Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        |mut upgrade: Upgrade<_>| async move {
            server_pong(&mut upgrade, PING_ROUNDS).await.unwrap();
        },
    )
    .await;

    let client = h3_client(&server);
    let conn = client
        .post(server.base_url())
        .with_http_version(Version::Http3)
        .upgrade()
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http3);
    let mut upgrade = Upgrade::from(conn);
    client_ping(&mut upgrade, PING_ROUNDS).await?;

    server.shut_down().await;
    Ok(())
}

// ----- prelude body before upgrade (`Body::keep_open`) -----
//
// An initial request/response body is sent without terminating the stream; the peer
// reads response headers and both sides then continue a framed bidi exchange over the
// resulting `Upgrade`. This is the lifecycle state the old "upgrade discards the body"
// path couldn't express. h1 only for now — the h2/h3 variants land with their send-path
// changes. Both directions use a fixed-length (`&str`) prelude on purpose: `keep_open`
// re-sources it through the chunked path, so the headline `with_body("…").upgrade()`
// ergonomics work without hand-wrapping a length-less streaming body.

#[test(harness)]
async fn h1_client_prelude_body_then_bidi() -> TestResult {
    let (tx, rx) = async_channel::bounded(1);
    let server = H1Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        move |mut upgrade: Upgrade<_>| {
            let tx = tx.clone();
            async move {
                // The prelude the client sent before the upgrade arrives on the inbound
                // side as the leading bytes of the (still-open) request body.
                let prelude = read_line_async(&mut upgrade)
                    .await
                    .unwrap()
                    .expect("client prelude before EOF");
                tx.send(prelude).await.unwrap();
                server_pong_to_eof(upgrade).await.unwrap();
            }
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client
        .post(server.base_url())
        .with_body("prelude\n")
        .upgrade()
        .await?;
    assert_eq!(conn.status().unwrap(), 200);

    let mut upgrade = Upgrade::from(conn);
    assert_eq!(rx.recv().await?, "prelude\n");

    client_ping(&mut upgrade, PING_ROUNDS).await?;
    upgrade.close().await?;
    let mut drained = Vec::new();
    upgrade.read_to_end(&mut drained).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h1_server_prelude_body_then_bidi() -> TestResult {
    let server = H1Server::with_upgrade(
        |conn: Conn<_>| async move {
            conn.upgrade()
                .with_status(200)
                .with_response_body("server-prelude\n")
        },
        |upgrade: Upgrade<_>| async move {
            server_pong_to_eof(upgrade).await.unwrap();
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client.post(server.base_url()).upgrade().await?;
    assert_eq!(conn.status().unwrap(), 200);

    let mut upgrade = Upgrade::from(conn);
    // The server's prelude arrives as the leading bytes of the (still-open) response body.
    let prelude = read_line_async(&mut upgrade)
        .await?
        .expect("server prelude before EOF");
    assert_eq!(prelude, "server-prelude\n");

    client_ping(&mut upgrade, PING_ROUNDS).await?;
    upgrade.close().await?;
    let mut drained = Vec::new();
    upgrade.read_to_end(&mut drained).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h2c_client_prelude_body_then_bidi() -> TestResult {
    let (tx, rx) = async_channel::bounded(1);
    let server = H2cServer::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        move |mut upgrade: Upgrade<_>| {
            let tx = tx.clone();
            async move {
                let prelude = read_line_async(&mut upgrade)
                    .await
                    .unwrap()
                    .expect("client prelude before EOF");
                tx.send(prelude).await.unwrap();
                server_pong(&mut upgrade, PING_ROUNDS).await.unwrap();
            }
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client
        .post(server.base_url())
        .with_http_version(Version::Http2)
        .with_body("prelude\n")
        .upgrade()
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http2);

    let mut upgrade = Upgrade::from(conn);
    assert_eq!(rx.recv().await?, "prelude\n");
    client_ping(&mut upgrade, PING_ROUNDS).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h2c_server_prelude_body_then_bidi() -> TestResult {
    let server = H2cServer::with_upgrade(
        |conn: Conn<_>| async move {
            conn.upgrade()
                .with_status(200)
                .with_response_body("server-prelude\n")
        },
        |mut upgrade: Upgrade<_>| async move {
            server_pong(&mut upgrade, PING_ROUNDS).await.unwrap();
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client
        .post(server.base_url())
        .with_http_version(Version::Http2)
        .upgrade()
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http2);

    let mut upgrade = Upgrade::from(conn);
    let prelude = read_line_async(&mut upgrade)
        .await?
        .expect("server prelude before EOF");
    assert_eq!(prelude, "server-prelude\n");
    client_ping(&mut upgrade, PING_ROUNDS).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h3_client_prelude_body_then_bidi() -> TestResult {
    let (tx, rx) = async_channel::bounded(1);
    let server = H3Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        move |mut upgrade: Upgrade<_>| {
            let tx = tx.clone();
            async move {
                let prelude = read_line_async(&mut upgrade)
                    .await
                    .unwrap()
                    .expect("client prelude before EOF");
                tx.send(prelude).await.unwrap();
                server_pong(&mut upgrade, PING_ROUNDS).await.unwrap();
            }
        },
    )
    .await;

    let client = h3_client(&server);
    let conn = client
        .post(server.base_url())
        .with_http_version(Version::Http3)
        .with_body("prelude\n")
        .upgrade()
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http3);

    let mut upgrade = Upgrade::from(conn);
    assert_eq!(rx.recv().await?, "prelude\n");
    client_ping(&mut upgrade, PING_ROUNDS).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h3_server_prelude_body_then_bidi() -> TestResult {
    let server = H3Server::with_upgrade(
        |conn: Conn<_>| async move {
            conn.upgrade()
                .with_status(200)
                .with_response_body("server-prelude\n")
        },
        |mut upgrade: Upgrade<_>| async move {
            server_pong(&mut upgrade, PING_ROUNDS).await.unwrap();
        },
    )
    .await;

    let client = h3_client(&server);
    let conn = client
        .post(server.base_url())
        .with_http_version(Version::Http3)
        .upgrade()
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http3);

    let mut upgrade = Upgrade::from(conn);
    let prelude = read_line_async(&mut upgrade)
        .await?
        .expect("server prelude before EOF");
    assert_eq!(prelude, "server-prelude\n");
    client_ping(&mut upgrade, PING_ROUNDS).await?;

    server.shut_down().await;
    Ok(())
}

// ----- streaming (unknown-length) prelude bodies -----
//
// The tests above use fixed-length (`&str`/`Static`) preludes. These use an unknown-length
// streaming body — the realistic gRPC shape — which hits different framing code: on h1 and h3
// the `Streaming` arm of `Body::write_into` frames the payload instead of the `Static` arm,
// and on h2 the `Streaming` branch of `into_h2`. h1 is covered both directions (one
// `keep_open` framing each); h2c and h3 are covered client-side (the Streaming arm is shared
// with the server send path, which the fixed-length server tests above already exercise).

fn streaming_prelude(bytes: &'static [u8]) -> Body {
    Body::new_streaming(Cursor::new(bytes), None)
}

#[test(harness)]
async fn h1_client_streaming_prelude_then_bidi() -> TestResult {
    let (tx, rx) = async_channel::bounded(1);
    let server = H1Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        move |mut upgrade: Upgrade<_>| {
            let tx = tx.clone();
            async move {
                let prelude = read_line_async(&mut upgrade)
                    .await
                    .unwrap()
                    .expect("client prelude before EOF");
                tx.send(prelude).await.unwrap();
                server_pong_to_eof(upgrade).await.unwrap();
            }
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client
        .post(server.base_url())
        .with_body(streaming_prelude(b"prelude\n"))
        .upgrade()
        .await?;
    assert_eq!(conn.status().unwrap(), 200);

    let mut upgrade = Upgrade::from(conn);
    assert_eq!(rx.recv().await?, "prelude\n");
    client_ping(&mut upgrade, PING_ROUNDS).await?;
    upgrade.close().await?;
    let mut drained = Vec::new();
    upgrade.read_to_end(&mut drained).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h1_server_streaming_prelude_then_bidi() -> TestResult {
    let server = H1Server::with_upgrade(
        |conn: Conn<_>| async move {
            conn.upgrade()
                .with_status(200)
                .with_response_body(streaming_prelude(b"server-prelude\n"))
        },
        |upgrade: Upgrade<_>| async move {
            server_pong_to_eof(upgrade).await.unwrap();
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client.post(server.base_url()).upgrade().await?;
    assert_eq!(conn.status().unwrap(), 200);

    let mut upgrade = Upgrade::from(conn);
    let prelude = read_line_async(&mut upgrade)
        .await?
        .expect("server prelude before EOF");
    assert_eq!(prelude, "server-prelude\n");
    client_ping(&mut upgrade, PING_ROUNDS).await?;
    upgrade.close().await?;
    let mut drained = Vec::new();
    upgrade.read_to_end(&mut drained).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h2c_client_streaming_prelude_then_bidi() -> TestResult {
    let (tx, rx) = async_channel::bounded(1);
    let server = H2cServer::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        move |mut upgrade: Upgrade<_>| {
            let tx = tx.clone();
            async move {
                let prelude = read_line_async(&mut upgrade)
                    .await
                    .unwrap()
                    .expect("client prelude before EOF");
                tx.send(prelude).await.unwrap();
                server_pong(&mut upgrade, PING_ROUNDS).await.unwrap();
            }
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client
        .post(server.base_url())
        .with_http_version(Version::Http2)
        .with_body(streaming_prelude(b"prelude\n"))
        .upgrade()
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http2);

    let mut upgrade = Upgrade::from(conn);
    assert_eq!(rx.recv().await?, "prelude\n");
    client_ping(&mut upgrade, PING_ROUNDS).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h3_client_streaming_prelude_then_bidi() -> TestResult {
    let (tx, rx) = async_channel::bounded(1);
    let server = H3Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        move |mut upgrade: Upgrade<_>| {
            let tx = tx.clone();
            async move {
                let prelude = read_line_async(&mut upgrade)
                    .await
                    .unwrap()
                    .expect("client prelude before EOF");
                tx.send(prelude).await.unwrap();
                server_pong(&mut upgrade, PING_ROUNDS).await.unwrap();
            }
        },
    )
    .await;

    let client = h3_client(&server);
    let conn = client
        .post(server.base_url())
        .with_http_version(Version::Http3)
        .with_body(streaming_prelude(b"prelude\n"))
        .upgrade()
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http3);

    let mut upgrade = Upgrade::from(conn);
    assert_eq!(rx.recv().await?, "prelude\n");
    client_ping(&mut upgrade, PING_ROUNDS).await?;

    server.shut_down().await;
    Ok(())
}

/// Captured inbound-side state from a server upgrade handler.
type InboundCapture = (Vec<u8>, Option<Headers>);

/// Server upgrade handler that reads the request body to EOF through `Upgrade` and
/// forwards the body + decoded trailers across `tx` for the test to assert. Covers the
/// "server inbound trailers" matrix cell — confirms that trailing HEADERS land in
/// `Upgrade::received_trailers` after `read_to_end`.
async fn capture_inbound<T>(mut upgrade: Upgrade<T>, tx: async_channel::Sender<InboundCapture>)
where
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    let mut body = Vec::new();
    upgrade
        .read_to_end(&mut body)
        .await
        .expect("server Upgrade read_to_end");
    let trailers = upgrade.received_trailers().cloned();
    let _ = tx.send((body, trailers)).await;
}

fn assert_inbound(received: InboundCapture, expected_body: &[u8]) {
    let (body, trailers) = received;
    assert_eq!(body, expected_body, "inbound body");
    let trailers = trailers.expect("server should decode trailing HEADERS into received_trailers");
    assert_eq!(trailers.get_str("x-client-trailer"), Some("client-value"));
    assert_eq!(trailers.get_str("grpc-status"), Some("0"));
}

fn client_request_trailers() -> Headers {
    let mut trailers = Headers::new();
    trailers.insert("x-client-trailer", "client-value");
    trailers.insert("grpc-status", "0");
    trailers
}

#[test(harness)]
async fn h1_inbound_trailers_round_trip() -> TestResult {
    let (tx, rx) = async_channel::bounded(1);
    let server = H1Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        move |upgrade: Upgrade<_>| {
            let tx = tx.clone();
            async move { capture_inbound(upgrade, tx).await }
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client.post(server.base_url()).upgrade().await?;
    assert_eq!(conn.status().unwrap(), 200);

    let mut upgrade = Upgrade::from(conn);
    upgrade.write_all(b"client request body").await?;
    upgrade.send_trailers(client_request_trailers()).await?;

    assert_inbound(rx.recv().await?, b"client request body");

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h2c_inbound_trailers_round_trip() -> TestResult {
    let (tx, rx) = async_channel::bounded(1);
    let server = H2cServer::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        move |upgrade: Upgrade<_>| {
            let tx = tx.clone();
            async move { capture_inbound(upgrade, tx).await }
        },
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client
        .post(server.base_url())
        .with_http_version(Version::Http2)
        .upgrade()
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http2);

    let mut upgrade = Upgrade::from(conn);
    upgrade.write_all(b"client request body").await?;
    upgrade.send_trailers(client_request_trailers()).await?;

    assert_inbound(rx.recv().await?, b"client request body");

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h3_inbound_trailers_round_trip() -> TestResult {
    let (tx, rx) = async_channel::bounded(1);
    let server = H3Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        move |upgrade: Upgrade<_>| {
            let tx = tx.clone();
            async move { capture_inbound(upgrade, tx).await }
        },
    )
    .await;

    let client = h3_client(&server);
    let conn = client
        .post(server.base_url())
        .with_http_version(Version::Http3)
        .upgrade()
        .await?;

    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http3);

    let mut upgrade = Upgrade::from(conn);
    upgrade.write_all(b"client request body").await?;
    upgrade.send_trailers(client_request_trailers()).await?;

    assert_inbound(rx.recv().await?, b"client request body");

    server.shut_down().await;
    Ok(())
}

/// Server upgrade handler that writes a fixed body + trailers via `Upgrade` —
/// shared between the three "client reads outbound trailers via Upgrade" tests.
async fn server_write_body_and_trailers<T>(mut upgrade: Upgrade<T>)
where
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    upgrade.write_all(b"hello framed").await.unwrap();
    let mut trailers = Headers::new();
    trailers.insert("x-result", "ok");
    upgrade.send_trailers(trailers).await.unwrap();
}

/// Drive the client side of a "server writes body+trailers, client reads via
/// Upgrade" round-trip and assert what came back. Covers the "client inbound
/// trailers" matrix cell — the parallel of the existing `*_outbound_framing_and_trailers`
/// tests but verifying the client-side `Upgrade` decoder, not the standard
/// `trillium-client` response-body path.
async fn assert_client_reads_outbound_trailers(conn: trillium_client::Conn) -> TestResult {
    let mut upgrade = Upgrade::from(conn);
    let mut received = Vec::new();
    upgrade.read_to_end(&mut received).await?;
    assert_eq!(received, b"hello framed");
    let trailers = upgrade
        .received_trailers()
        .expect("client Upgrade should populate received_trailers after read_to_end");
    assert_eq!(trailers.get_str("x-result"), Some("ok"));
    Ok(())
}

#[test(harness)]
async fn h1_outbound_trailers_via_client_upgrade() -> TestResult {
    let server = H1Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        server_write_body_and_trailers,
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client.get(server.base_url()).upgrade().await?;

    assert_eq!(conn.status().unwrap(), 200);

    assert_client_reads_outbound_trailers(conn).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h2c_outbound_trailers_via_client_upgrade() -> TestResult {
    let server = H2cServer::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        server_write_body_and_trailers,
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client
        .get(server.base_url())
        .with_http_version(Version::Http2)
        .upgrade()
        .await?;

    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http2);

    assert_client_reads_outbound_trailers(conn).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h3_outbound_trailers_via_client_upgrade() -> TestResult {
    let server = H3Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        server_write_body_and_trailers,
    )
    .await;

    let client = h3_client(&server);
    let conn = client
        .get(server.base_url())
        .with_http_version(Version::Http3)
        .upgrade()
        .await?;

    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http3);

    assert_client_reads_outbound_trailers(conn).await?;

    server.shut_down().await;
    Ok(())
}

/// Server upgrade handler that goes straight to `send_trailers` with no prior body
/// writes — exercises the "first wire byte is the chunked terminator / trailing HEADERS"
/// shape (grpc-shaped responses with status-only trailers and no message body).
async fn server_send_trailers_only<T>(upgrade: Upgrade<T>)
where
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    let mut trailers = Headers::new();
    trailers.insert("x-result", "ok");
    upgrade.send_trailers(trailers).await.unwrap();
}

async fn assert_empty_body_with_ok_trailer(mut conn: trillium_client::Conn) -> TestResult {
    assert_eq!(conn.response_body().read_string().await?, "");
    assert_eq!(
        conn.response_trailers().and_then(|t| t.get_str("x-result")),
        Some("ok"),
    );
    Ok(())
}

#[test(harness)]
async fn h1_empty_body_trailers_only() -> TestResult {
    let server = H1Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        server_send_trailers_only,
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client.get(server.base_url()).await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_empty_body_with_ok_trailer(conn).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h2c_empty_body_trailers_only() -> TestResult {
    let server = H2cServer::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        server_send_trailers_only,
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client
        .get(server.base_url())
        .with_http_version(Version::Http2)
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http2);
    assert_empty_body_with_ok_trailer(conn).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h3_empty_body_trailers_only() -> TestResult {
    let server = H3Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        server_send_trailers_only,
    )
    .await;

    let client = h3_client(&server);
    let conn = client
        .get(server.base_url())
        .with_http_version(Version::Http3)
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http3);
    assert_empty_body_with_ok_trailer(conn).await?;

    server.shut_down().await;
    Ok(())
}

const MULTI_FRAME_CHUNK_COUNT: usize = 50;
const MULTI_FRAME_CHUNK_SIZE: usize = 100;

fn multi_frame_chunk(i: usize) -> Vec<u8> {
    vec![b'a' + (i as u8 % 26); MULTI_FRAME_CHUNK_SIZE]
}

/// Server upgrade handler that writes `MULTI_FRAME_CHUNK_COUNT` body chunks via separate
/// `write_all` calls (each one a distinct h1 chunk / h3 DATA frame), then trailers.
/// Exercises frame-boundary correctness — the trailing HEADERS parser has to skip past
/// many DATA frames on h3, and the chunked decoder has to assemble many chunks on h1.
async fn server_write_multi_frame_body_and_trailers<T>(mut upgrade: Upgrade<T>)
where
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    for i in 0..MULTI_FRAME_CHUNK_COUNT {
        upgrade.write_all(&multi_frame_chunk(i)).await.unwrap();
    }
    let mut trailers = Headers::new();
    trailers.insert("x-chunks", MULTI_FRAME_CHUNK_COUNT.to_string());
    upgrade.send_trailers(trailers).await.unwrap();
}

async fn assert_multi_frame_body_and_trailers(mut conn: trillium_client::Conn) -> TestResult {
    let body = conn.response_body().read_bytes().await?;
    assert_eq!(
        body.len(),
        MULTI_FRAME_CHUNK_COUNT * MULTI_FRAME_CHUNK_SIZE,
        "assembled body length"
    );
    for (i, chunk) in body.chunks(MULTI_FRAME_CHUNK_SIZE).enumerate() {
        assert_eq!(chunk, multi_frame_chunk(i), "chunk {i} content");
    }
    let chunks_str = MULTI_FRAME_CHUNK_COUNT.to_string();
    assert_eq!(
        conn.response_trailers().and_then(|t| t.get_str("x-chunks")),
        Some(chunks_str.as_str()),
    );
    Ok(())
}

#[test(harness)]
async fn h1_multi_frame_body_with_trailers() -> TestResult {
    let server = H1Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        server_write_multi_frame_body_and_trailers,
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client.get(server.base_url()).await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_multi_frame_body_and_trailers(conn).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h2c_multi_frame_body_with_trailers() -> TestResult {
    let server = H2cServer::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        server_write_multi_frame_body_and_trailers,
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client
        .get(server.base_url())
        .with_http_version(Version::Http2)
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http2);
    assert_multi_frame_body_and_trailers(conn).await?;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h3_multi_frame_body_with_trailers() -> TestResult {
    let server = H3Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        server_write_multi_frame_body_and_trailers,
    )
    .await;

    let client = h3_client(&server);
    let conn = client
        .get(server.base_url())
        .with_http_version(Version::Http3)
        .await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http3);
    assert_multi_frame_body_and_trailers(conn).await?;

    server.shut_down().await;
    Ok(())
}

/// Upgrade handler that drops the upgrade without writing or closing — exercises the
/// teardown path on each protocol (h1 TCP FIN on drop, h2 `H2Transport::Drop` RST,
/// h3 quinn stream reset). The test's invariant is "client read returns in finite
/// time"; the actual return shape (Ok with empty body for h1's clean EOF, Err for
/// h2/h3 stream resets) is protocol-intrinsic and not asserted.
async fn server_drop_without_close<T>(upgrade: Upgrade<T>) {
    drop(upgrade);
}

#[test(harness)]
async fn h1_server_drop_without_close_does_not_hang() -> TestResult {
    let server = H1Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        server_drop_without_close,
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client.get(server.base_url()).upgrade().await?;

    assert_eq!(conn.status().unwrap(), 200);

    let mut upgrade = Upgrade::from(conn);
    let mut received = Vec::new();
    let _ = upgrade.read_to_end(&mut received).await;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h2c_server_drop_without_close_does_not_hang() -> TestResult {
    let server = H2cServer::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        server_drop_without_close,
    )
    .await;

    let client = Client::new(trillium_smol::ClientConfig::default());
    let conn = client
        .get(server.base_url())
        .with_http_version(Version::Http2)
        .upgrade()
        .await?;

    assert_eq!(conn.status().unwrap(), 200);

    let mut upgrade = Upgrade::from(conn);
    let mut received = Vec::new();
    let _ = upgrade.read_to_end(&mut received).await;

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h3_server_drop_without_close_does_not_hang() -> TestResult {
    let server = H3Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(200) },
        server_drop_without_close,
    )
    .await;

    let client = h3_client(&server);
    let conn = client
        .get(server.base_url())
        .with_http_version(Version::Http3)
        .upgrade()
        .await?;

    assert_eq!(conn.status().unwrap(), 200);

    let mut upgrade = Upgrade::from(conn);
    let mut received = Vec::new();
    let _ = upgrade.read_to_end(&mut received).await;

    server.shut_down().await;
    Ok(())
}

/// A browser's WebSocket handshake is a bodyless `GET`: it declares neither
/// `Transfer-Encoding` nor `Content-Length`. Such a request parses as an empty body
/// (`ReceivedBodyState::End`), and that framing must not survive into the `Upgrade` — past
/// the 101 the transport is a raw byte stream. Inheriting it made the first server-side read
/// return EOF while the peer was still connected, so a proxy relaying the upgrade would
/// immediately tear down a healthy connection.
///
/// Driven over a raw socket on purpose: `trillium-client`'s `.upgrade()` sends a streaming
/// body, so it always declares chunked framing and never exercises this path.
#[test(harness)]
async fn h1_upgrade_without_declared_framing_reads_raw() -> TestResult {
    use async_net::TcpStream as RawStream;

    let server = H1Server::with_upgrade(
        |conn: Conn<_>| async move { conn.upgrade().with_status(101) },
        |upgrade: Upgrade<_>| async move {
            server_pong_to_eof(upgrade).await.unwrap();
        },
    )
    .await;

    let addr = server
        .base_url()
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    let mut socket = RawStream::connect(&*addr).await?;

    // Exactly what a browser sends: no Transfer-Encoding, no Content-Length.
    socket
        .write_all(
            format!(
                "GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: Upgrade\r\nUpgrade: \
                 websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: \
                 SXDK9rww/7aGBCD6Bml0BQ==\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;

    // Drain the response head up to the blank line.
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        assert_eq!(socket.read(&mut byte).await?, 1, "EOF during response head");
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    assert!(
        head.starts_with("HTTP/1.1 101"),
        "unexpected head: {head:?}"
    );

    // The upgraded stream is raw: the server must still be reading, not at EOF.
    socket.write_all(b"ping: 0\n").await?;
    let echoed = read_line_async(&mut socket)
        .await?
        .expect("server read EOF on an upgraded socket that is still open");
    assert_eq!(echoed, "pong: ping: 0\n");

    server.shut_down().await;
    Ok(())
}
