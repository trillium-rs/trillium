#![cfg(quinn_testing)]

use rcgen::generate_simple_self_signed;
use std::{io, net::SocketAddr, sync::Arc};
use trillium::{Conn, KnownHeaderName};
use trillium_client::{Client, Version};
use trillium_quinn::{ClientQuicConfig, QuicConfig};
use trillium_rustls::{RustlsConfig, rustls};
use trillium_testing::{TestResult, client_config, config, harness, test};

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
    let handle = config()
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

/// Build an `Arc<dyn ResolvesServerCert>` that always returns the test cert.
fn static_cert_resolver(tc: &TestCert) -> Arc<dyn rustls::server::ResolvesServerCert> {
    let certs: Vec<_> = rustls_pemfile::certs(&mut io::BufReader::new(&tc.cert_pem[..]))
        .collect::<Result<_, _>>()
        .unwrap();
    let key = rustls_pemfile::private_key(&mut io::BufReader::new(&tc.key_pem[..]))
        .unwrap()
        .unwrap();
    let certified_key = rustls::sign::CertifiedKey::from_der(
        certs,
        key,
        &rustls::crypto::aws_lc_rs::default_provider(),
    )
    .unwrap();
    Arc::new(rustls::sign::SingleCertAndKey::from(certified_key))
}

/// Build a trillium client configured for both H1 (TLS) and H3 with the test cert trusted.
fn trillium_client(tc: &TestCert) -> Client {
    Client::new_with_quic(
        RustlsConfig::new(rustls_client_config(tc), client_config()),
        ClientQuicConfig::from_rustls_client_config(rustls_client_config(tc)),
    )
}

// ---------------------------------------------------------------------------
// Tests: trillium client against trillium server
// ---------------------------------------------------------------------------

#[test(harness)]
async fn trillium_client_h3_basic() -> TestResult {
    let tc = test_cert();
    let addr = start_server(|conn: Conn| async move { conn.ok("hello from h3") }, &tc).await;

    let client = trillium_client(&tc);
    let mut conn = client
        .get(format!("https://localhost:{}/", addr.port()))
        .with_http_version(Version::Http3)
        .await?;

    assert_eq!(conn.status().unwrap(), 200u16);
    assert_eq!(conn.response_body().read_string().await?, "hello from h3");
    Ok(())
}

#[test(harness)]
async fn trillium_client_h3_post_with_body() -> TestResult {
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
        .await?;

    assert_eq!(conn.status().unwrap(), 200u16);
    assert_eq!(conn.response_body().read_string().await?, "got: sent body");
    Ok(())
}

#[test(harness)]
async fn trillium_client_h3_large_response() -> TestResult {
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
        .await?;

    assert_eq!(conn.status().unwrap(), 200u16);
    assert_eq!(conn.response_body().read_string().await?, large_body);
    Ok(())
}

#[test(harness)]
async fn trillium_client_alt_svc_upgrade() -> TestResult {
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
    let mut first = client.get(base_url.as_str()).await?;
    assert_eq!(first.status().unwrap(), 200u16);
    let version_str = first.response_body().read_string().await?;
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
    let mut second = client.get(base_url.as_str()).await?;
    assert_eq!(second.status().unwrap(), 200u16);
    let version_str = second.response_body().read_string().await?;
    assert!(
        version_str.contains("Http3"),
        "expected H3 for second request after alt-svc, got: {version_str}"
    );
    Ok(())
}

#[test(harness)]
async fn trillium_client_h3_via_cert_resolver() -> TestResult {
    // Verifies QuicConfig::from_cert_resolver wires a rustls cert resolver through to a
    // live H3 handshake. The static resolver here stands in for a dynamic source like ACME.
    let tc = test_cert();
    let handle = config()
        .with_port(0)
        .with_host("localhost")
        .without_signals()
        .with_acceptor(trillium_rustls::RustlsAcceptor::from_single_cert(
            &tc.cert_pem,
            &tc.key_pem,
        ))
        .with_quic(QuicConfig::from_cert_resolver(static_cert_resolver(&tc)))
        .spawn(|conn: Conn| async move { conn.ok("hello via resolver") });
    let addr = *handle.info().await.tcp_socket_addr().unwrap();

    let client = trillium_client(&tc);
    let mut conn = client
        .get(format!("https://localhost:{}/", addr.port()))
        .with_http_version(Version::Http3)
        .await?;

    assert_eq!(conn.status().unwrap(), 200u16);
    assert_eq!(
        conn.response_body().read_string().await?,
        "hello via resolver"
    );
    Ok(())
}
