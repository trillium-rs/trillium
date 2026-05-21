//! End-to-end tests for the trillium-client WebSocket-over-h3 client (RFC 9220 extended
//! CONNECT) against a trillium server with the websocket handler installed over an HTTP/3
//! QUIC endpoint.

use futures_lite::StreamExt;
use trillium::Handler;
use trillium_client::{
    Client, Version, WebSocketConn,
    websocket::{self, Message},
};
use trillium_quinn::{ClientQuicConfig, QuicConfig};
use trillium_rustls::{
    RustlsAcceptor, RustlsConfig,
    rustls::{self, RootCertStore},
};
use trillium_testing::{TestResult, harness, test};
use trillium_websockets::websocket;

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

fn rustls_client_config(cert: &TestCert) -> rustls::ClientConfig {
    let mut roots = RootCertStore::empty();
    roots.add(cert.cert_der.clone()).unwrap();
    rustls::ClientConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth()
}

fn quic_client(cert: &TestCert) -> Client {
    Client::new_with_quic(
        RustlsConfig::new(
            rustls_client_config(cert),
            trillium_smol::ClientConfig::default(),
        ),
        ClientQuicConfig::from_rustls_client_config(rustls_client_config(cert)),
    )
}

fn echo_websocket() -> impl Handler {
    websocket(|mut conn: WebSocketConn| async move {
        while let Some(Ok(Message::Text(input))) = conn.next().await {
            conn.send_string(format!("echo:{input}"))
                .await
                .expect("send_string");
        }
    })
}

#[test(harness)]
async fn websocket_over_h3() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();

    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .with_acceptor(RustlsAcceptor::from_single_cert(
            &cert.cert_pem,
            &cert.key_pem,
        ))
        .with_quic(QuicConfig::from_single_cert(&cert.cert_pem, &cert.key_pem))
        .spawn(echo_websocket());
    let port = server.info().await.tcp_socket_addr().unwrap().port();

    let client = quic_client(&cert).with_base(format!("https://localhost:{port}"));

    let mut ws = client
        .get("/")
        .with_http_version(Version::Http3)
        .into_websocket()
        .await?;

    ws.send_string("hello h3".into()).await?;
    let response = ws.next().await.expect("response")?;
    assert_eq!(response, Message::text("echo:hello h3"));

    ws.send_string("again".into()).await?;
    let response = ws.next().await.expect("response")?;
    assert_eq!(response, Message::text("echo:again"));

    server.shut_down().await;
    Ok(())
}

/// A server without a websocket handler doesn't advertise `SETTINGS_ENABLE_CONNECT_PROTOCOL`,
/// so the h3 extended-CONNECT bootstrap surfaces `ExtendedConnectUnsupported`.
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
        .with_quic(QuicConfig::from_single_cert(&cert.cert_pem, &cert.key_pem))
        .spawn("plain http server");
    let port = server.info().await.tcp_socket_addr().unwrap().port();

    let client = quic_client(&cert).with_base(format!("https://localhost:{port}"));

    let err = client
        .get("/")
        .with_http_version(Version::Http3)
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
