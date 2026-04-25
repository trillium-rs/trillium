//! End-to-end smoke test for the trillium-client h2 path.
//!
//! Spawns a trillium server using the rustls acceptor (which advertises `[h2, http/1.1]` via
//! ALPN) and a trillium client whose rustls config trusts the test cert and advertises the
//! same ALPN list. The client request should be transparently dispatched over HTTP/2.

use std::sync::Arc;
use trillium::Conn;
use trillium_client::{Client, Version};
use trillium_rustls::{
    RustlsAcceptor, RustlsConfig,
    rustls::{ClientConfig, RootCertStore},
};
use trillium_testing::{TestResult, harness, test};

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

async fn version_handler(conn: Conn) -> Conn {
    let version = conn.http_version();
    conn.ok(format!("{version:?}"))
}

async fn echo_handler(mut conn: Conn) -> Conn {
    let body = conn.request_body_string().await.unwrap_or_default();
    let version = conn.http_version();
    conn.ok(format!("{version:?}:{body}"))
}

#[test(harness)]
async fn alpn_negotiates_h2() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();

    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .with_acceptor(RustlsAcceptor::from_single_cert(
            &cert.cert_pem,
            &cert.key_pem,
        ))
        .spawn(version_handler);
    let info = server.info().await;
    let port = info.tcp_socket_addr().unwrap().port();

    let client = Client::new(RustlsConfig::new(
        Arc::new(rustls_client_config(&cert)),
        trillium_smol::ClientConfig::default(),
    ))
    .with_base(format!("https://localhost:{port}"));

    // Two consecutive requests: the first promotes the TCP connection to h2 and pools it; the
    // second reuses the same `H2Connection` via `try_exec_h2_pooled`.
    for _ in 0..2 {
        let mut conn = client.get("/").await?;
        assert_eq!(conn.status().unwrap(), 200);
        assert_eq!(conn.http_version(), Version::Http2);
        assert_eq!(conn.response_body().read_string().await?, "Http2");
    }

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn h2_post_with_body() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();

    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .with_acceptor(RustlsAcceptor::from_single_cert(
            &cert.cert_pem,
            &cert.key_pem,
        ))
        .spawn(echo_handler);
    let info = server.info().await;
    let port = info.tcp_socket_addr().unwrap().port();

    let client = Client::new(RustlsConfig::new(
        Arc::new(rustls_client_config(&cert)),
        trillium_smol::ClientConfig::default(),
    ))
    .with_base(format!("https://localhost:{port}"));

    let mut conn = client.post("/").with_body("hello h2").await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http2);
    assert_eq!(conn.response_body().read_string().await?, "Http2:hello h2");

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn without_http2_forces_h1() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();

    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .with_acceptor(RustlsAcceptor::from_single_cert(
            &cert.cert_pem,
            &cert.key_pem,
        ))
        .spawn(version_handler);
    let info = server.info().await;
    let port = info.tcp_socket_addr().unwrap().port();

    let client = Client::new(
        RustlsConfig::new(
            Arc::new(rustls_client_config(&cert)),
            trillium_smol::ClientConfig::default(),
        )
        .without_http2(),
    )
    .with_base(format!("https://localhost:{port}"));

    let mut conn = client.get("/").await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http1_1);
    assert_eq!(conn.response_body().read_string().await?, "Http1_1");

    server.shut_down().await;
    Ok(())
}
