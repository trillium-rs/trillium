//! End-to-end smoke test for the trillium-client h2c (cleartext HTTP/2) prior-knowledge path.
//!
//! Spawns a trillium server with no TLS acceptor — the server discovers h2c by sniffing the
//! 24-byte client connection preface — and a trillium client with `http_version` set to
//! `Version::Http2` on an `http://` URL, which signals "speak h2 immediately, don't even try
//! h1." The server's preface sniffer matches and dispatches to the h2 path.

use trillium::Conn;
use trillium_client::{Client, Version};
use trillium_testing::{TestResult, harness, test};

async fn version_handler(conn: Conn) -> Conn {
    let version = conn.http_version();
    conn.ok(format!("{version:?}"))
}

#[test(harness)]
async fn http2_hint_over_cleartext_uses_h2c() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();

    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .spawn(version_handler);
    let info = server.info().await;
    let port = info.tcp_socket_addr().unwrap().port();

    let client = Client::new(trillium_smol::ClientConfig::default())
        .with_base(format!("http://localhost:{port}"));

    // Two consecutive requests: the first opens a fresh h2c connection and pools it; the
    // second reuses the same `H2Connection` via `try_exec_h2_pooled`.
    for _ in 0..2 {
        let mut conn = client.get("/").with_http_version(Version::Http2).await?;
        assert_eq!(conn.status().unwrap(), 200);
        assert_eq!(conn.http_version(), Version::Http2);
        assert_eq!(conn.response_body().read_string().await?, "Http2");
    }

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn no_hint_over_cleartext_uses_h1() -> TestResult {
    let _ = env_logger::builder().is_test(true).try_init();

    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .spawn(version_handler);
    let info = server.info().await;
    let port = info.tcp_socket_addr().unwrap().port();

    let client = Client::new(trillium_smol::ClientConfig::default())
        .with_base(format!("http://localhost:{port}"));

    // Default (no hint) over cleartext stays on h1 — there's no h2c probing without an
    // explicit `Version::Http2` opt-in.
    let mut conn = client.get("/").await?;
    assert_eq!(conn.status().unwrap(), 200);
    assert_eq!(conn.http_version(), Version::Http1_1);
    assert_eq!(conn.response_body().read_string().await?, "Http1_1");

    server.shut_down().await;
    Ok(())
}
