//! An HTTP/3 response that fails partway — its body errors, or its handler panics — must reach the
//! client as a failed stream, not as a complete (truncated) response; the connection keeps serving
//! other requests.

use futures_lite::AsyncRead;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use trillium::Conn;
use trillium_client::{Body, Client, Version};
use trillium_quinn::{ClientQuicConfig, QuicConfig};
use trillium_rustls::{
    RustlsAcceptor, RustlsConfig,
    rustls::{self, RootCertStore},
};
use trillium_testing::{TestResult, harness, test};

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

/// Yields this many bytes, then fails. Enough that some of the body is flushed to the wire before
/// the failure, so a client could mistake what arrived for the whole body.
struct FailsAfter(usize);

impl AsyncRead for FailsAfter {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.0 == 0 {
            return Poll::Ready(Err(io::Error::other("body source failed")));
        }
        let n = self.0.min(buf.len());
        buf[..n].fill(b'x');
        self.0 -= n;
        Poll::Ready(Ok(n))
    }
}

async fn handler(conn: Conn) -> Conn {
    match conn.path() {
        "/panic" => panic!("handler panicked"),
        "/fails" => conn
            .with_status(200)
            .with_body(Body::new_streaming(FailsAfter(100 * 1024), None))
            .halt(),
        _ => conn.ok("fine"),
    }
}

#[test(harness)]
async fn failed_h3_responses_reach_the_client_as_failures() -> TestResult {
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
        .spawn(handler);
    let port = server.info().await.tcp_socket_addr().unwrap().port();
    let client = quic_client(&cert).with_base(format!("https://localhost:{port}"));

    // The reset can overtake the response headers, so the failure may surface on the request or
    // on the body; either way it must not read as a complete response.
    let body = async {
        let mut conn = client
            .get("/fails")
            .with_http_version(Version::Http3)
            .await?;
        conn.response_body().read_string().await
    }
    .await;
    assert!(
        body.is_err(),
        "a failed body read as complete ({:?} bytes)",
        body.map(|body| body.len())
    );

    let panicked = client.get("/panic").with_http_version(Version::Http3).await;
    assert!(panicked.is_err(), "a panicked handler's request succeeded");

    let mut conn = client.get("/").with_http_version(Version::Http3).await?;
    assert_eq!(conn.http_version(), Version::Http3);
    assert_eq!(conn.response_body().read_string().await?, "fine");

    server.shut_down().await;
    Ok(())
}
