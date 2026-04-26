//! End-to-end tests for the trillium-client WebSocket-over-h2 client (RFC 8441 §4 extended
//! CONNECT).
//!
//! Mirrors `h2_alpn.rs` for transport setup; the websocket-specific bits exercise the
//! `Conn::into_websocket()` extended-CONNECT path against a trillium server with the websocket
//! handler installed.

use futures_lite::StreamExt;
use std::sync::Arc;
use trillium_client::{
    Client, Version, WebSocketConn,
    websocket::{self, Message},
};
use trillium_rustls::{
    RustlsAcceptor, RustlsConfig,
    rustls::{ClientConfig, RootCertStore},
};
use trillium_testing::{TestResult, harness, test};
use trillium_websockets::websocket;

struct TestCert {
    cert_pem: Vec<u8>,
    key_pem: Vec<u8>,
    cert_der: trillium_rustls::rustls::pki_types::CertificateDer<'static>,
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

fn rustls_client_config(cert: &TestCert) -> ClientConfig {
    let mut roots = RootCertStore::empty();
    roots.add(cert.cert_der.clone()).unwrap();
    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    config
}

fn echo_websocket() -> impl trillium::Handler {
    websocket(|mut conn: WebSocketConn| async move {
        while let Some(Ok(Message::Text(input))) = conn.next().await {
            conn.send_string(format!("echo:{input}"))
                .await
                .expect("send_string");
        }
    })
}

#[test(harness)]
async fn websocket_over_h2() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();

    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .with_acceptor(RustlsAcceptor::from_single_cert(
            &cert.cert_pem,
            &cert.key_pem,
        ))
        .spawn(echo_websocket());
    let info = server.info().await;
    let port = info.tcp_socket_addr().unwrap().port();

    let client = Client::new(RustlsConfig::new(
        Arc::new(rustls_client_config(&cert)),
        trillium_smol::ClientConfig::default(),
    ))
    .with_base(format!("https://localhost:{port}"));

    let mut ws = client
        .get("/")
        .with_http_version(Version::Http2)
        .into_websocket()
        .await?;

    ws.send_string("hello h2".into()).await?;
    let response = ws.next().await.expect("response")?;
    assert_eq!(response, Message::text("echo:hello h2"));

    server.shut_down().await;
    Ok(())
}

/// Calling `into_websocket` on a conn that's already been awaited surfaces
/// `ErrorKind::AlreadyExecuted` rather than silently misbehaving. This guards the contract
/// that `into_websocket` *is* the execution; the user shouldn't drive the conn separately.
#[test(harness)]
async fn into_websocket_after_execution_is_an_error() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();

    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .with_acceptor(RustlsAcceptor::from_single_cert(
            &cert.cert_pem,
            &cert.key_pem,
        ))
        .spawn("not a websocket");
    let info = server.info().await;
    let port = info.tcp_socket_addr().unwrap().port();

    let client = Client::new(RustlsConfig::new(
        Arc::new(rustls_client_config(&cert)),
        trillium_smol::ClientConfig::default(),
    ))
    .with_base(format!("https://localhost:{port}"));

    let conn = client.get("/").await?;
    let err = conn.into_websocket().await.expect_err("expected error");
    assert!(matches!(err.kind, websocket::ErrorKind::AlreadyExecuted));

    server.shut_down().await;
    Ok(())
}

/// Server doesn't have a websocket handler → no `SETTINGS_ENABLE_CONNECT_PROTOCOL`
/// advertised → client surfaces `ExtendedConnectUnsupported`.
#[test(harness)]
async fn extended_connect_unsupported_when_server_lacks_setting() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();

    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .with_acceptor(RustlsAcceptor::from_single_cert(
            &cert.cert_pem,
            &cert.key_pem,
        ))
        .spawn("plain http server");
    let info = server.info().await;
    let port = info.tcp_socket_addr().unwrap().port();

    let client = Client::new(RustlsConfig::new(
        Arc::new(rustls_client_config(&cert)),
        trillium_smol::ClientConfig::default(),
    ))
    .with_base(format!("https://localhost:{port}"));

    let err = client
        .get("/")
        .with_http_version(Version::Http2)
        .into_websocket()
        .await
        .expect_err("expected ExtendedConnectUnsupported");
    assert!(matches!(
        err.kind,
        websocket::ErrorKind::ExtendedConnectUnsupported,
    ));

    server.shut_down().await;
    Ok(())
}
