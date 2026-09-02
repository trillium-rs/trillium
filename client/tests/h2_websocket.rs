//! End-to-end tests for the trillium-client WebSocket-over-h2 client (RFC 8441 §4 extended
//! CONNECT).
//!
//! Mirrors `h2_alpn.rs` for transport setup; the websocket-specific bits exercise the
//! `Conn::into_websocket()` extended-CONNECT path against a trillium server with the websocket
//! handler installed.

use futures_lite::StreamExt;
use std::sync::{Arc, Mutex};
use trillium::{Handler, Info};
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

/// Records the http version of every request that reaches the handler chain, so a test can
/// assert which transport the client's upgrade actually arrived on.
#[derive(Clone, Default)]
struct RecordVersion(Arc<Mutex<Vec<Version>>>);

impl Handler for RecordVersion {
    async fn run(&self, conn: trillium::Conn) -> trillium::Conn {
        self.0.lock().unwrap().push(conn.http_version());
        conn
    }
}

impl RecordVersion {
    fn versions(&self) -> Vec<Version> {
        self.0.lock().unwrap().clone()
    }
}

/// Undoes the websocket handler's `SETTINGS_ENABLE_CONNECT_PROTOCOL` advertisement, leaving a
/// server that upgrades websockets over h1 but not over h2 — the shape of most deployed
/// servers, and the one the client's h1 fallback exists for. Must come *after* the websocket
/// handler in the tuple so its `init` runs last.
struct WithoutExtendedConnect;

impl Handler for WithoutExtendedConnect {
    async fn run(&self, conn: trillium::Conn) -> trillium::Conn {
        conn
    }

    async fn init(&mut self, info: &mut Info) {
        info.config_mut().set_extended_connect_enabled(false);
    }
}

fn spawn(handler: impl Handler, cert: &TestCert) -> (trillium_server_common::ServerHandle, Client) {
    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .with_acceptor(RustlsAcceptor::from_single_cert(
            &cert.cert_pem,
            &cert.key_pem,
        ))
        .spawn(handler);
    (
        server,
        Client::new(RustlsConfig::new(
            Arc::new(rustls_client_config(cert)),
            trillium_smol::ClientConfig::default(),
        )),
    )
}

/// No version hint: ALPN negotiates h2, the peer advertises extended CONNECT, and the upgrade
/// goes out as an RFC 8441 CONNECT on that connection — no h1 detour.
#[test(harness)]
async fn websocket_auto_discovers_h2() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();
    let versions = RecordVersion::default();
    let (server, client) = spawn((versions.clone(), echo_websocket()), &cert);
    let port = server.info().await.tcp_socket_addr().unwrap().port();
    let client = client.with_base(format!("https://localhost:{port}"));

    let mut ws = client.get("/").into_websocket().await?;
    ws.send_string("hello".into()).await?;
    assert_eq!(
        ws.next().await.expect("response")?,
        Message::text("echo:hello")
    );
    assert_eq!(versions.versions(), [Version::Http2]);

    server.shut_down().await;
    Ok(())
}

/// No version hint, and ALPN negotiates h2 with a peer that does *not* advertise extended
/// CONNECT: the upgrade falls back to a fresh HTTP/1.1 connection rather than failing.
#[test(harness)]
async fn websocket_falls_back_to_h1_from_cold_h2() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();
    let versions = RecordVersion::default();
    let (server, client) = spawn(
        (versions.clone(), echo_websocket(), WithoutExtendedConnect),
        &cert,
    );
    let port = server.info().await.tcp_socket_addr().unwrap().port();
    let client = client.with_base(format!("https://localhost:{port}"));

    let mut ws = client.get("/").into_websocket().await?;
    ws.send_string("hello".into()).await?;
    assert_eq!(
        ws.next().await.expect("response")?,
        Message::text("echo:hello")
    );
    // The gate runs before any HEADERS go out, so the server saw only the h1 upgrade, and the
    // h2 connection it opened on the way is still pooled for ordinary requests.
    let plain = client.get("/").await?;
    assert_eq!(plain.http_version(), Version::Http2);
    drop(plain);
    assert_eq!(versions.versions(), [Version::Http1_1, Version::Http2]);

    server.shut_down().await;
    Ok(())
}

/// Same fallback when the h2 connection was already pooled by an earlier request: the pooled
/// connection is checked and skipped, the upgrade goes out over h1, and the h2 connection is
/// still there for ordinary requests afterwards.
#[test(harness)]
async fn websocket_falls_back_to_h1_from_pooled_h2() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();
    let versions = RecordVersion::default();
    let (server, client) = spawn(
        (versions.clone(), echo_websocket(), WithoutExtendedConnect),
        &cert,
    );
    let port = server.info().await.tcp_socket_addr().unwrap().port();
    let client = client.with_base(format!("https://localhost:{port}"));

    let plain = client.get("/").await?;
    assert_eq!(plain.http_version(), Version::Http2);
    drop(plain);

    let mut ws = client.get("/").into_websocket().await?;
    ws.send_string("hello".into()).await?;
    assert_eq!(
        ws.next().await.expect("response")?,
        Message::text("echo:hello")
    );

    let plain = client.get("/").await?;
    assert_eq!(plain.http_version(), Version::Http2);
    drop(plain);

    assert_eq!(
        versions.versions(),
        [Version::Http2, Version::Http1_1, Version::Http2]
    );

    server.shut_down().await;
    Ok(())
}

/// An `Http2` hint is where to start, not where to stop: an unsupporting peer is retried over
/// h1.
#[test(harness)]
async fn hinted_h2_falls_back_to_h1() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();
    let versions = RecordVersion::default();
    let (server, client) = spawn(
        (versions.clone(), echo_websocket(), WithoutExtendedConnect),
        &cert,
    );
    let port = server.info().await.tcp_socket_addr().unwrap().port();
    let client = client.with_base(format!("https://localhost:{port}"));

    let mut ws = client
        .get("/")
        .with_http_version(Version::Http2)
        .into_websocket()
        .await?;
    ws.send_string("hello".into()).await?;
    assert_eq!(
        ws.next().await.expect("response")?,
        Message::text("echo:hello")
    );
    assert_eq!(versions.versions(), [Version::Http1_1]);

    server.shut_down().await;
    Ok(())
}

/// Strict mode turns the retry into an error, whether set on the conn or as the client default.
#[test(harness)]
async fn strict_does_not_fall_back() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();
    let cert = test_cert();
    let (server, client) = spawn((echo_websocket(), WithoutExtendedConnect), &cert);
    let port = server.info().await.tcp_socket_addr().unwrap().port();
    let client = client.with_base(format!("https://localhost:{port}"));

    let err = client
        .get("/")
        .with_strict_http_version()
        .into_websocket()
        .await
        .expect_err("expected ExtendedConnectUnsupported");
    assert!(matches!(
        err.kind,
        websocket::ErrorKind::ExtendedConnectUnsupported
    ));

    let err = client
        .clone()
        .with_strict_http_version()
        .get("/")
        .with_http_version(Version::Http2)
        .into_websocket()
        .await
        .expect_err("expected ExtendedConnectUnsupported");
    assert!(matches!(
        err.kind,
        websocket::ErrorKind::ExtendedConnectUnsupported
    ));

    server.shut_down().await;
    Ok(())
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

    drop(err);
    server.shut_down().await;
    Ok(())
}

/// Server doesn't have a websocket handler → no `SETTINGS_ENABLE_CONNECT_PROTOCOL`
/// advertised → a strict client surfaces `ExtendedConnectUnsupported`.
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
        .with_strict_http_version()
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
