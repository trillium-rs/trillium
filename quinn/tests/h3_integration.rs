use rcgen::generate_simple_self_signed;
use std::net::SocketAddr;
use trillium::{Conn, KnownHeaderName};
use trillium_client::{Client, Version};
use trillium_quinn::{ClientQuicConfig, QuicConfig};
use trillium_rustls::{RustlsConfig, rustls};
use trillium_tokio::ClientConfig;

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

struct TestCert {
    cert_pem: Vec<u8>,
    key_pem: Vec<u8>,
    cert_der: rustls::pki_types::CertificateDer<'static>,
}

fn test_cert() -> TestCert {
    let rcgen::CertifiedKey {
        cert, signing_key, ..
    } = generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    TestCert {
        cert_pem: cert.pem().into_bytes(),
        key_pem: signing_key.serialize_pem().into_bytes(),
        cert_der: cert.der().clone(),
    }
}

fn rustls_client_config(tc: &TestCert) -> rustls::ClientConfig {
    let mut root_store = rustls::RootCertStore::empty();
    root_store.add(tc.cert_der.clone()).unwrap();
    rustls::ClientConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(root_store)
    .with_no_client_auth()
}

/// Start a trillium server with TLS (H1) and QUIC (H3) on the same port.
/// The server runs as a background task until the runtime shuts down.
async fn start_server(handler: impl trillium::Handler, tc: &TestCert) -> SocketAddr {
    let handle = trillium_tokio::config()
        .with_port(0)
        .with_host("localhost")
        .without_signals()
        .with_acceptor(trillium_rustls::RustlsAcceptor::from_single_cert(
            &tc.cert_pem,
            &tc.key_pem,
        ))
        .with_quic(QuicConfig::from_single_cert(&tc.cert_pem, &tc.key_pem))
        .spawn(handler);
    *handle.info().await.tcp_socket_addr().unwrap()
}

/// Build a trillium client configured for both H1 (TLS) and H3 with the test cert trusted.
fn trillium_client(tc: &TestCert) -> Client {
    Client::new_with_quic(
        RustlsConfig::new(rustls_client_config(tc), ClientConfig::default()),
        ClientQuicConfig::from_rustls_client_config(rustls_client_config(tc)),
    )
}

// ---------------------------------------------------------------------------
// Tests: trillium client against trillium server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn trillium_client_h3_basic() {
    let tc = test_cert();
    let addr = start_server(|conn: Conn| async move { conn.ok("hello from h3") }, &tc).await;

    let client = trillium_client(&tc);
    let mut conn = client
        .get(format!("https://localhost:{}/", addr.port()))
        .with_http_version(Version::Http3)
        .await
        .unwrap();

    assert_eq!(conn.status().unwrap(), 200u16);
    assert_eq!(
        conn.response_body().read_string().await.unwrap(),
        "hello from h3"
    );
}

#[tokio::test]
async fn trillium_client_h3_post_with_body() {
    let tc = test_cert();
    let addr = start_server(
        |mut conn: Conn| async move {
            let body = conn.request_body_string().await.unwrap();
            conn.ok(format!("got: {body}"))
        },
        &tc,
    )
    .await;

    let client = trillium_client(&tc);
    let mut conn = client
        .post(format!("https://localhost:{}/", addr.port()))
        .with_http_version(Version::Http3)
        .with_body("sent body")
        .await
        .unwrap();

    assert_eq!(conn.status().unwrap(), 200u16);
    assert_eq!(
        conn.response_body().read_string().await.unwrap(),
        "got: sent body"
    );
}

#[tokio::test]
async fn trillium_client_h3_large_response() {
    let tc = test_cert();
    let large_body: String = "y".repeat(1024 * 64);
    let addr = start_server(
        {
            let large_body = large_body.clone();
            move |conn: Conn| {
                let large_body = large_body.clone();
                async move { conn.ok(large_body) }
            }
        },
        &tc,
    )
    .await;

    let client = trillium_client(&tc);
    let mut conn = client
        .get(format!("https://localhost:{}/", addr.port()))
        .with_http_version(Version::Http3)
        .await
        .unwrap();

    assert_eq!(conn.status().unwrap(), 200u16);
    assert_eq!(
        conn.response_body().read_string().await.unwrap(),
        large_body
    );
}

#[tokio::test]
async fn trillium_client_alt_svc_upgrade() {
    // Verify the full alt-svc discovery flow: H1 response teaches the client
    // about H3, and the next request uses H3 automatically.
    let tc = test_cert();
    let addr = start_server(
        |conn: Conn| async move {
            let version = format!("{:?}", conn.http_version());
            conn.ok(version)
        },
        &tc,
    )
    .await;

    let client = trillium_client(&tc);
    let base_url = format!("https://localhost:{}/", addr.port());

    // First request: H1, response carries Alt-Svc header
    let mut first = client.get(base_url.as_str()).await.unwrap();
    assert_eq!(first.status().unwrap(), 200u16);
    let version_str = first.response_body().read_string().await.unwrap();
    assert!(
        version_str.contains("Http1"),
        "expected H1 for first request, got: {version_str}"
    );
    assert!(
        first
            .response_headers()
            .get_str(KnownHeaderName::AltSvc)
            .is_some(),
        "server should advertise Alt-Svc"
    );

    // Second request: client should upgrade to H3
    let mut second = client.get(base_url.as_str()).await.unwrap();
    assert_eq!(second.status().unwrap(), 200u16);
    let version_str = second.response_body().read_string().await.unwrap();
    assert!(
        version_str.contains("Http3"),
        "expected H3 for second request after alt-svc, got: {version_str}"
    );
}
