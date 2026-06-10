use super::{connection_is_upgrade, websocket};
use crate::WebSocketConn;
use trillium::{Conn, Status};
use trillium_testing::{TestServer, harness, test};

const SAMPLE_KEY: &str = "dGhlIHNhbXBsZSBub25jZQ==";

#[test(harness)]
async fn rejects_unsupported_version() {
    let app = TestServer::new(websocket(|_: WebSocketConn| async {})).await;

    // RFC 6455 §4.4: an unsupported version aborts the handshake with 426 and advertises 13,
    // rather than switching protocols.
    app.get("/")
        .with_request_header("connection", "Upgrade")
        .with_request_header("upgrade", "websocket")
        .with_request_header("sec-websocket-key", SAMPLE_KEY)
        .with_request_header("sec-websocket-version", "99")
        .await
        .assert_status(Status::UpgradeRequired)
        .assert_header("sec-websocket-version", "13");

    // A missing version is likewise not version 13.
    app.get("/")
        .with_request_header("connection", "Upgrade")
        .with_request_header("upgrade", "websocket")
        .with_request_header("sec-websocket-key", SAMPLE_KEY)
        .await
        .assert_status(Status::UpgradeRequired);

    // Version 13 negotiates normally.
    app.get("/")
        .with_request_header("connection", "Upgrade")
        .with_request_header("upgrade", "websocket")
        .with_request_header("sec-websocket-key", SAMPLE_KEY)
        .with_request_header("sec-websocket-version", "13")
        .await
        .assert_status(Status::SwitchingProtocols);
}

#[test(harness)]
async fn ignores_non_get_handshake() {
    let app = TestServer::new(websocket(|_: WebSocketConn| async {})).await;
    app.post("/")
        .with_request_header("connection", "Upgrade")
        .with_request_header("upgrade", "websocket")
        .with_request_header("sec-websocket-key", SAMPLE_KEY)
        .with_request_header("sec-websocket-version", "13")
        .await
        .assert_status(Status::NotFound);
}

#[test(harness)]
async fn test_connection_is_upgrade() {
    let handler = |conn: Conn| async move {
        if connection_is_upgrade(&conn) {
            conn.ok("upgrade")
        } else {
            conn.ok("no-upgrade")
        }
    };

    let app = TestServer::new(handler).await;

    app.get("/").await.assert_ok().assert_body("no-upgrade");

    app.get("/")
        .with_request_header("connection", "keep-alive, Upgrade")
        .await
        .assert_ok()
        .assert_body("upgrade");

    app.get("/")
        .with_request_header("connection", "upgrade")
        .await
        .assert_ok()
        .assert_body("upgrade");

    app.get("/")
        .with_request_header("connection", "UPgrAde")
        .await
        .assert_ok()
        .assert_body("upgrade");

    app.get("/")
        .with_request_header("connection", "UPgrAde, keep-alive")
        .await
        .assert_ok()
        .assert_body("upgrade");

    app.get("/")
        .with_request_header("connection", "keep-alive")
        .await
        .assert_ok()
        .assert_body("no-upgrade");
}
