//! Full-stack integration tests for trillium-server-common's cleartext HTTP/2 dispatch,
//! run under the tokio runtime so hyper's `h2` crate can be used natively without the
//! `async_compat::Compat` bridge.
//!
//! Covers phase-5 work: the 24-byte preface peek in `running_config::handle_stream` and its
//! two outcomes — preface match → HTTP/2 driver, mismatch → HTTP/1 parser with the peeked
//! bytes handed in via `trillium_http::run_with_initial_bytes`.
//!
//! These tests exist as scaffolding while the trillium-rs client is being taught HTTP/2;
//! once that lands, they'll be replaced by smoke tests that use the trillium client against
//! the trillium server. The hyper-h2 dependency here is not part of a long-term testing
//! strategy.

use h2::client;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use trillium::{Conn, Version};
use trillium_tokio::config;

async fn version_handler(conn: Conn) -> Conn {
    let version = conn.http_version();
    conn.ok(format!("{version:?}"))
}

/// Cleartext HTTP/2 prior-knowledge: client sends the 24-byte preface as the first bytes on
/// the TCP connection. `running_config` peeks the preface, confirms the match, and routes
/// the transport to the h2 driver. The handler sees a `Conn` with `version == Http2`.
#[tokio::test]
async fn cleartext_prior_knowledge_dispatches_to_h2() {
    let _ = env_logger::builder().is_test(true).try_init();
    let handle = config()
        .with_host("localhost")
        .with_port(0)
        .spawn(version_handler);
    let info = handle.info().await;
    let addr = *info.tcp_socket_addr().expect("listener bound");

    let tcp = TcpStream::connect(addr).await.expect("tcp connect");
    let (mut send, connection) = client::handshake(tcp).await.expect("h2 handshake");
    // Client's Connection future resolves once the server closes the TCP stream. A tidy
    // graceful close (server emits GOAWAY and waits for peer FIN) is phase-6 work; today
    // we emit GOAWAY and drop the socket, which the peer typically surfaces as an IO
    // error — acceptable here, since the dispatch assertion above is what the test is
    // checking.
    let conn_task = tokio::spawn(async move {
        let _ = connection.await;
    });

    let request = http::Request::builder()
        .method(http::Method::GET)
        .uri("/")
        .body(())
        .unwrap();
    let (response_fut, _) = send.send_request(request, true).unwrap();
    let response = response_fut.await.expect("response headers");
    assert_eq!(response.status(), http::StatusCode::OK);

    let mut body = response.into_body();
    let mut collected = Vec::new();
    while let Some(chunk) = body.data().await {
        collected.extend_from_slice(&chunk.expect("data chunk"));
    }
    assert_eq!(
        collected.as_slice(),
        format!("{:?}", Version::Http2).as_bytes(),
        "handler should have seen Http2"
    );

    drop(send);
    // hyper's `h2` client does not initiate graceful shutdown on `SendRequest` drop — its
    // `Connection` future sits waiting for a peer GOAWAY. Shutting down the server first
    // emits that GOAWAY, which lets the client task finish promptly.
    handle.shut_down().await;
    conn_task.await.expect("client conn task panicked");
}

/// Cleartext HTTP/1 fallback: client sends a regular `GET /` as its first bytes. Those
/// bytes do not match the HTTP/2 preface, so `running_config` hands them into the h1 parser
/// via `run_with_initial_bytes`. The handler sees a `Conn` with `version == Http1_1`.
#[tokio::test]
async fn cleartext_non_preface_dispatches_to_h1() {
    let _ = env_logger::builder().is_test(true).try_init();
    let handle = config()
        .with_host("localhost")
        .with_port(0)
        .spawn(version_handler);
    let info = handle.info().await;
    let addr = *info.tcp_socket_addr().expect("listener bound");

    let mut tcp = TcpStream::connect(addr).await.expect("tcp connect");
    tcp.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    tcp.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8_lossy(&response);
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n"),
        "expected HTTP/1 response, got: {response}"
    );
    let expected_body = format!("{:?}", Version::Http1_1);
    assert!(
        response.ends_with(&expected_body),
        "expected body {expected_body:?} in response tail: {response}"
    );

    handle.shut_down().await;
}

/// Short (< 24 bytes) HTTP/1 request: the preface peek must bail on the first non-matching
/// byte rather than waiting for more bytes that will never come. `GET / HTTP/1.0\r\n\r\n`
/// is 18 bytes — a valid HTTP/1.0 request (no Host required), well under the 24-byte
/// preface length. If the peek loop were to read-until-24, the test would hang.
#[tokio::test]
async fn short_h1_request_does_not_block_peek() {
    async fn simple_handler(conn: Conn) -> Conn {
        conn.ok("short")
    }

    let _ = env_logger::builder().is_test(true).try_init();
    let handle = config()
        .with_host("localhost")
        .with_port(0)
        .spawn(simple_handler);
    let info = handle.info().await;
    let addr = *info.tcp_socket_addr().expect("listener bound");

    let mut tcp = TcpStream::connect(addr).await.expect("tcp connect");
    tcp.write_all(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();

    let mut response = Vec::new();
    tcp.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8_lossy(&response);
    assert!(
        response.starts_with("HTTP/1.1 200 OK\r\n") || response.starts_with("HTTP/1.0 200 OK\r\n"),
        "expected HTTP/1 response, got: {response}"
    );
    assert!(
        response.ends_with("short"),
        "unexpected response tail: {response}"
    );

    handle.shut_down().await;
}
