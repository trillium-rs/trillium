//! End-to-end test that the trillium-client owned-body path (`take_response_body`) surfaces
//! HTTP/3 response trailers. The owned `ResponseBody` must carry the h3 protocol session, or
//! the trailing HEADERS frame is decoded into nothing and the trailers are silently dropped.
//! Reads them back via `ResponseBody::trailers`.

use futures_lite::{AsyncRead, AsyncReadExt, io::Cursor};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};
use trillium::Conn;
use trillium_client::{Body, BodySource, Client, Headers, Version};
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

struct BodyWithTrailers {
    cursor: Cursor<Vec<u8>>,
    trailers: Option<Headers>,
}

impl BodyWithTrailers {
    fn new(body: impl Into<Vec<u8>>, trailers: Headers) -> Self {
        Self {
            cursor: Cursor::new(body.into()),
            trailers: Some(trailers),
        }
    }
}

impl AsyncRead for BodyWithTrailers {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().cursor).poll_read(cx, buf)
    }
}

impl BodySource for BodyWithTrailers {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        self.get_mut().trailers.take()
    }
}

fn one_trailer(name: &'static str, value: &'static str) -> Headers {
    let mut h = Headers::new();
    h.insert(name, value);
    h
}

async fn trailer_handler(mut conn: Conn) -> Conn {
    let body = conn.request_body_string().await.unwrap_or_default();
    let ping = conn
        .request_trailers()
        .and_then(|t| t.get_str("x-ping"))
        .unwrap_or("")
        .to_string();
    let resp_trailers = one_trailer("x-pong", "pong");
    conn.with_status(200)
        .with_body(Body::new_with_trailers(
            BodyWithTrailers::new(format!("{ping}:{body}"), resp_trailers),
            None,
        ))
        .halt()
}

#[test(harness)]
async fn h3_owned_body_trailers() -> TestResult {
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
        .spawn(trailer_handler);
    let port = server.info().await.tcp_socket_addr().unwrap().port();

    let client = quic_client(&cert).with_base(format!("https://localhost:{port}"));

    let req_trailers = one_trailer("x-ping", "ping");
    let mut conn = client
        .post("/")
        .with_http_version(Version::Http3)
        .with_body(Body::new_with_trailers(
            BodyWithTrailers::new("data", req_trailers),
            None,
        ))
        .await?;

    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http3);

    let mut body = conn.take_response_body().expect("response body");
    let mut read = String::new();
    body.read_to_string(&mut read).await?;
    assert_eq!(read, "ping:data");
    assert_eq!(
        body.trailers().and_then(|t| t.get_str("x-pong")),
        Some("pong")
    );

    server.shut_down().await;
    Ok(())
}
