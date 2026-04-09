use bytes::{Buf, Bytes};
use h3::client::SendRequest;
use http::{Method, Request};
use rcgen::generate_simple_self_signed;
use std::{net::SocketAddr, sync::Arc};
use trillium::{Body, Conn, KnownHeaderName};
use trillium_client::{Client, Version};
use trillium_quinn::{ClientQuicConfig, QuicConfig};
use trillium_rustls::RustlsConfig;
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

/// Build a raw h3-quinn client that trusts the test cert.
async fn h3_raw_client(
    addr: SocketAddr,
    tc: &TestCert,
) -> SendRequest<h3_quinn::OpenStreams, Bytes> {
    let mut tls_config = rustls_client_config(tc);
    tls_config.alpn_protocols = vec![b"h3".to_vec()];

    let quic_tls = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(tls_config)).unwrap();
    let quinn_client_config = quinn::ClientConfig::new(Arc::new(quic_tls));

    let local_addr: SocketAddr = if addr.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    }
    .parse()
    .unwrap();
    let mut endpoint = quinn::Endpoint::client(local_addr).unwrap();
    endpoint.set_default_client_config(quinn_client_config);

    let quinn_conn = endpoint.connect(addr, "localhost").unwrap().await.unwrap();
    let h3_conn = h3_quinn::Connection::new(quinn_conn);
    let (driver, send_request) = h3::client::builder()
        .build::<_, _, Bytes>(h3_conn)
        .await
        .unwrap();

    tokio::spawn(async move {
        let mut driver = driver;
        let e = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
        eprintln!("h3 driver closed: {e}");
    });

    send_request
}

/// Build a trillium client configured for both H1 (TLS) and H3 with the test cert trusted.
fn trillium_client(tc: &TestCert) -> Client {
    Client::new_with_quic(
        RustlsConfig::new(rustls_client_config(tc), ClientConfig::default()),
        ClientQuicConfig::from_rustls_client_config(rustls_client_config(tc)),
    )
}

// ---------------------------------------------------------------------------
// Helpers for raw h3 requests
// ---------------------------------------------------------------------------

async fn h3_get(
    send_request: &mut SendRequest<h3_quinn::OpenStreams, Bytes>,
    uri: &str,
) -> (http::StatusCode, http::HeaderMap, String) {
    let request = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(())
        .unwrap();

    let mut stream = send_request.send_request(request).await.unwrap();
    stream.finish().await.unwrap();

    let response = stream.recv_response().await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();

    let mut body_bytes = Vec::new();
    while let Some(chunk) = stream.recv_data().await.unwrap() {
        body_bytes.extend_from_slice(chunk.chunk());
    }

    (status, headers, String::from_utf8(body_bytes).unwrap())
}

// ---------------------------------------------------------------------------
// Tests: raw h3 client against trillium server
// ---------------------------------------------------------------------------

#[tokio::test]
async fn h3_empty_response() {
    let tc = test_cert();
    let addr = start_server(|conn: Conn| async move { conn.ok("") }, &tc).await;
    let mut client = h3_raw_client(addr, &tc).await;
    let (status, _, body) = h3_get(&mut client, "http://localhost/").await;
    assert_eq!(status, 200);
    assert_eq!(body, "");
}

#[tokio::test]
async fn h3_static_response_body() {
    let tc = test_cert();
    let addr = start_server(|conn: Conn| async move { conn.ok("hello world") }, &tc).await;
    let mut client = h3_raw_client(addr, &tc).await;
    let (status, _, body) = h3_get(&mut client, "http://localhost/").await;
    assert_eq!(status, 200);
    assert_eq!(body, "hello world");
}

#[tokio::test]
async fn h3_response_status_codes() {
    let tc = test_cert();
    let addr = start_server(
        |conn: Conn| async move { conn.with_status(404).with_body("not found").halt() },
        &tc,
    )
    .await;
    let mut client = h3_raw_client(addr, &tc).await;
    let (status, _, body) = h3_get(&mut client, "http://localhost/").await;
    assert_eq!(status, 404);
    assert_eq!(body, "not found");
}

#[tokio::test]
async fn h3_response_headers() {
    let tc = test_cert();
    let addr = start_server(
        |conn: Conn| async move {
            conn.ok("ok")
                .with_response_header("x-custom", "header-value")
        },
        &tc,
    )
    .await;
    let mut client = h3_raw_client(addr, &tc).await;
    let (status, headers, _) = h3_get(&mut client, "http://localhost/").await;
    assert_eq!(status, 200);
    assert_eq!(headers.get("x-custom").unwrap(), "header-value");
}

#[tokio::test]
async fn h3_content_length_matches_body() {
    let tc = test_cert();
    let body = "exactly this long";
    let addr = start_server(move |conn: Conn| async move { conn.ok(body) }, &tc).await;
    let mut client = h3_raw_client(addr, &tc).await;
    let (_, headers, received_body) = h3_get(&mut client, "http://localhost/").await;
    let content_length: usize = headers
        .get("content-length")
        .unwrap()
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(content_length, body.len());
    assert_eq!(received_body, body);
}

#[tokio::test]
async fn h3_large_response_body() {
    let tc = test_cert();
    let large_body: String = "x".repeat(1024 * 64); // 64 KiB
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
    let mut client = h3_raw_client(addr, &tc).await;
    let (status, _, body) = h3_get(&mut client, "http://localhost/").await;
    assert_eq!(status, 200);
    assert_eq!(body, large_body);
}

#[tokio::test]
async fn h3_concurrent_requests() {
    let tc = test_cert();
    let addr = start_server(
        |conn: Conn| async move {
            let n: u32 = conn.path().trim_start_matches('/').parse().unwrap_or(0);
            conn.ok(format!("response {n}"))
        },
        &tc,
    )
    .await;

    let mut client = h3_raw_client(addr, &tc).await;

    let mut streams = Vec::new();
    for i in 0..10u32 {
        let request = Request::builder()
            .method(Method::GET)
            .uri(format!("http://localhost/{i}"))
            .body(())
            .unwrap();
        streams.push((i, client.send_request(request).await.unwrap()));
    }

    for (i, mut stream) in streams {
        stream.finish().await.unwrap();
        let response = stream.recv_response().await.unwrap();
        assert_eq!(response.status(), 200);
        let mut body = Vec::new();
        while let Some(chunk) = stream.recv_data().await.unwrap() {
            body.extend_from_slice(chunk.chunk());
        }
        assert_eq!(String::from_utf8(body).unwrap(), format!("response {i}"));
    }
}

#[tokio::test]
async fn h3_request_body() {
    let tc = test_cert();
    let addr = start_server(
        |mut conn: Conn| async move {
            let body = conn.request_body_string().await.unwrap();
            conn.ok(format!("echo: {body}"))
        },
        &tc,
    )
    .await;

    let mut send_request = h3_raw_client(addr, &tc).await;

    let request = Request::builder()
        .method(Method::POST)
        .uri("http://localhost/")
        .header("content-type", "text/plain")
        .body(())
        .unwrap();

    let mut stream = send_request.send_request(request).await.unwrap();
    stream
        .send_data(Bytes::from("hello from client"))
        .await
        .unwrap();
    stream.finish().await.unwrap();

    let response = stream.recv_response().await.unwrap();
    assert_eq!(response.status(), 200);

    let mut body = Vec::new();
    while let Some(chunk) = stream.recv_data().await.unwrap() {
        body.extend_from_slice(chunk.chunk());
    }
    assert_eq!(String::from_utf8(body).unwrap(), "echo: hello from client");
}

#[tokio::test]
async fn h3_streaming_response_body() {
    let tc = test_cert();
    let addr = start_server(
        |conn: Conn| async move {
            conn.ok(Body::new_streaming(
                futures_lite::io::Cursor::new(b"streamed body".to_vec()),
                None,
            ))
        },
        &tc,
    )
    .await;
    let mut client = h3_raw_client(addr, &tc).await;
    let (status, _, body) = h3_get(&mut client, "http://localhost/").await;
    assert_eq!(status, 200);
    assert_eq!(body, "streamed body");
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
