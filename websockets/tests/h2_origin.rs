//! The origin check applies to the RFC 8441 extended-CONNECT handshake as well as the h1 one.
//!
//! Browsers use extended CONNECT whenever the connection is h2, so an h1-only check would stop
//! applying the moment an application moved behind an h2 proxy. The test transport doesn't speak
//! h2, so this binds a port and drives cleartext h2 (prior knowledge) through trillium-client.

use futures_lite::StreamExt;
use trillium_client::{
    Client, Version,
    websocket::{ErrorKind, Message},
};
use trillium_testing::{TestResult, harness, test};
use trillium_websockets::{WebSocket, WebSocketConn, websocket};

fn echo() -> impl trillium::Handler {
    websocket(|mut conn: WebSocketConn| async move {
        while let Some(Ok(Message::Text(input))) = conn.next().await {
            conn.send_string(format!("echo:{input}"))
                .await
                .expect("send_string");
        }
    })
}

async fn assert_h2_handshake(
    handler: impl trillium::Handler,
    origin: impl FnOnce(u16) -> Option<String>,
    expected: Result<(), trillium::Status>,
) -> TestResult {
    let server = trillium_smol::config()
        .with_host("localhost")
        .with_port(0)
        .spawn(handler);
    let port = server.info().await.tcp_socket_addr().unwrap().port();

    let client = Client::new(trillium_smol::ClientConfig::default())
        .with_base(format!("http://localhost:{port}"));

    let mut conn = client.get("/").with_http_version(Version::Http2);
    if let Some(origin) = origin(port) {
        conn.request_headers_mut().insert("origin", origin);
    }

    match (conn.into_websocket().await, expected) {
        (Ok(mut websocket), Ok(())) => {
            websocket.send_string("hello".into()).await?;
            assert_eq!(
                websocket.next().await.expect("response")?,
                Message::text("echo:hello")
            );
        }

        (Err(error), Err(status)) => match error.kind {
            ErrorKind::Status(actual) => assert_eq!(actual, status),
            other => panic!("expected status {status}, got {other}"),
        },

        (Ok(_), Err(status)) => panic!("expected the handshake to be rejected with {status}"),
        (Err(error), Ok(())) => panic!("expected the handshake to succeed, got {error}"),
    }

    server.shut_down().await;
    Ok(())
}

#[test(harness)]
async fn same_origin_default_allows_a_matching_origin() -> TestResult {
    assert_h2_handshake(
        echo(),
        |port| Some(format!("http://localhost:{port}")),
        Ok(()),
    )
    .await
}

#[test(harness)]
async fn same_origin_default_rejects_a_cross_origin_handshake() -> TestResult {
    assert_h2_handshake(
        echo(),
        |_| Some(String::from("https://evil.com")),
        Err(trillium::Status::Forbidden),
    )
    .await
}

#[test(harness)]
async fn absent_origin_is_allowed() -> TestResult {
    assert_h2_handshake(echo(), |_| None, Ok(())).await
}

#[test(harness)]
async fn allow_origins_applies_to_extended_connect() -> TestResult {
    assert_h2_handshake(
        WebSocket::new(|_: WebSocketConn| async {}).allow_origins(["https://app.example.com"]),
        |_| Some(String::from("https://evil.com")),
        Err(trillium::Status::Forbidden),
    )
    .await
}
