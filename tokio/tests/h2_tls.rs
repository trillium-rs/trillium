//! Full-stack integration tests for trillium-server-common's TLS + ALPN HTTP/2 dispatch.
//!
//! Covers phase-5 work: when a rustls acceptor negotiates `h2` via ALPN, the dispatcher
//! hands the TLS-wrapped transport directly to the HTTP/2 driver; if ALPN does not
//! negotiate `h2`, the same transport is handled as HTTP/1.
//!
//! Server-side uses `trillium-rustls`, which advertises `["h2", "http/1.1"]` via
//! `from_single_cert`. Client-side uses `tokio-rustls` so we can opt in to a specific
//! ALPN protocol list per test.
//!
//! These tests exist as scaffolding while the trillium-rs client is being taught HTTP/2;
//! once that lands, they'll be replaced by smoke tests using the trillium client.

use h2::client;
use rustls::pki_types::ServerName;
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::TlsConnector;
use trillium::{Conn, Version};
use trillium_rustls::RustlsAcceptor;
use trillium_server_common::ServerHandle;
use trillium_tokio::config;

async fn version_handler(conn: Conn) -> Conn {
    let version = conn.http_version();
    conn.ok(format!("{version:?}"))
}

/// Hermetic self-signed cert for `localhost`. Both halves (server PEMs and client-side
/// trust root DER) come from the same `rcgen::generate_simple_self_signed` call, so the
/// client trusts the exact cert the server presents.
struct TestCert {
    cert_pem: Vec<u8>,
    key_pem: Vec<u8>,
    cert_der: rustls::pki_types::CertificateDer<'static>,
}

fn test_cert() -> TestCert {
    let rcgen::CertifiedKey {
        cert, signing_key, ..
    } = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    TestCert {
        cert_pem: cert.pem().into_bytes(),
        key_pem: signing_key.serialize_pem().into_bytes(),
        cert_der: cert.der().clone(),
    }
}

async fn start_tls_server(cert: &TestCert) -> (ServerHandle, SocketAddr) {
    let handle = config()
        .with_host("localhost")
        .with_port(0)
        .with_acceptor(RustlsAcceptor::from_single_cert(
            &cert.cert_pem,
            &cert.key_pem,
        ))
        .spawn(version_handler);
    let info = handle.info().await;
    let addr = *info.tcp_socket_addr().expect("listener bound");
    (handle, addr)
}

fn rustls_client_config(cert: &TestCert, alpn: &[&[u8]]) -> rustls::ClientConfig {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.add(cert.cert_der.clone()).unwrap();
    let mut config = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(root_store)
    .with_no_client_auth();
    config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    config
}

/// Client advertises `h2` via ALPN; server negotiates h2; handler sees `Version::Http2`.
#[tokio::test]
async fn alpn_h2_dispatches_to_h2() {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();
    let (handle, addr) = start_tls_server(&cert).await;

    let tcp = TcpStream::connect(addr).await.expect("tcp connect");
    let connector = TlsConnector::from(Arc::new(rustls_client_config(&cert, &[b"h2"])));
    let domain = ServerName::try_from("localhost").unwrap();
    let tls = connector.connect(domain, tcp).await.expect("tls handshake");
    assert_eq!(
        tls.get_ref().1.alpn_protocol(),
        Some(b"h2" as &[u8]),
        "client/server should have negotiated h2 via ALPN"
    );

    let (mut send, connection) = client::handshake(tls).await.expect("h2 handshake");
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
    handle.shut_down().await;
    conn_task.await.expect("client conn task panicked");
}

/// Client advertises only `http/1.1` via ALPN; server negotiates http/1.1; handler sees
/// `Version::Http1_1`. Confirms the ALPN branch in `running_config::handle_stream` does not
/// route TLS connections to h2 when the peer didn't select it.
#[tokio::test]
async fn alpn_http1_dispatches_to_h1() {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();
    let (handle, addr) = start_tls_server(&cert).await;

    let tcp = TcpStream::connect(addr).await.expect("tcp connect");
    let connector = TlsConnector::from(Arc::new(rustls_client_config(&cert, &[b"http/1.1"])));
    let domain = ServerName::try_from("localhost").unwrap();
    let mut tls = connector.connect(domain, tcp).await.expect("tls handshake");
    assert_eq!(
        tls.get_ref().1.alpn_protocol(),
        Some(b"http/1.1" as &[u8]),
        "client/server should have negotiated http/1.1 via ALPN"
    );

    tls.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    // rustls surfaces peer close-without-`close_notify` as `UnexpectedEof`. Trillium's h1
    // path drops the socket after the response; the bytes we already collected are what
    // matters for the dispatch assertion.
    let mut response = Vec::new();
    match tls.read_to_end(&mut response).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {}
        Err(e) => panic!("unexpected tls read error: {e}"),
    }
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
