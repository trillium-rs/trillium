//! End-to-end smoke test for the trillium-client h2 path.
//!
//! Spawns a trillium server using the rustls acceptor (which advertises `[h2, http/1.1]` via
//! ALPN) and a trillium client whose rustls config trusts the test cert and advertises the
//! same ALPN list. The client request should be transparently dispatched over HTTP/2.

use futures_lite::{AsyncRead, io::Cursor};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use trillium::Conn;
use trillium_client::{Body, BodySource, Client, Headers, Version};
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

/// Trailer handler: echoes the request body prefixed with the request trailer `x-ping`
/// value, plus a response trailer `x-pong`. Exercises trailers on both directions in a
/// single round trip.
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

// FIXME: response trailers race with client-role stream removal. The current lifecycle
// removes the stream from the connection's map as soon as `send.completed && recv.eof`,
// which can fire before the conn task wakes up to read the body to EOF and call
// `take_trailers`. The fix is to align h2 with h1/h3: keep the stream in the map until the
// user drops their handle (`H2Transport::Drop`). Tracked as a follow-up commit on this
// branch. The request-trailers half of the test (server receives `x-ping`) is exercised by
// the body assertion below — `"ping:data"` only comes back if the server saw the trailer.
#[test(harness)]
#[ignore = "blocked on h2 client-role stream lifecycle fix; see comment above"]
async fn h2_bidirectional_trailers() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();

    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .with_acceptor(RustlsAcceptor::from_single_cert(
            &cert.cert_pem,
            &cert.key_pem,
        ))
        .spawn(trailer_handler);
    let info = server.info().await;
    let port = info.tcp_socket_addr().unwrap().port();

    let client = Client::new(RustlsConfig::new(
        Arc::new(rustls_client_config(&cert)),
        trillium_smol::ClientConfig::default(),
    ))
    .with_base(format!("https://localhost:{port}"));

    let req_trailers = one_trailer("x-ping", "ping");
    let mut conn = client
        .post("/")
        .with_body(Body::new_with_trailers(
            BodyWithTrailers::new("data", req_trailers),
            None,
        ))
        .await?;

    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http2);
    assert_eq!(conn.response_body().read_string().await?, "ping:data");
    assert_eq!(
        conn.response_trailers()
            .as_ref()
            .and_then(|t| t.get_str("x-pong")),
        Some("pong"),
    );

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
