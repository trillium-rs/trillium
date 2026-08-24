use super::{connection_is_upgrade, websocket};
use crate::{WebSocket, WebSocketConn, WebSocketHandler};
use trillium::{Conn, Handler, Status};
use trillium_testing::{ConnTest, TestServer, harness, test};

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

    // A `Connection` value split across multiple header lines coalesces into one token list
    // (RFC 9110 §5.6.1), so the `Upgrade` token is found even on a separate line from `keep-alive`.
    app.get("/")
        .with_request_header("connection", ["keep-alive", "Upgrade"])
        .await
        .assert_ok()
        .assert_body("upgrade");
}

fn handshake(app: &TestServer<impl Handler>) -> ConnTest {
    app.get("/")
        .with_request_header("connection", "Upgrade")
        .with_request_header("upgrade", "websocket")
        .with_request_header("sec-websocket-key", SAMPLE_KEY)
        .with_request_header("sec-websocket-version", "13")
}

async fn app(websocket: WebSocket<impl WebSocketHandler>) -> TestServer<impl Handler> {
    TestServer::new(websocket).await.with_host("example.com")
}

#[test(harness)]
async fn same_origin_is_the_default() {
    let app = app(websocket(|_: WebSocketConn| async {})).await;

    // no Origin at all: not a browser, nothing to check
    handshake(&app)
        .await
        .assert_status(Status::SwitchingProtocols);

    handshake(&app)
        .with_request_header("origin", "https://example.com")
        .await
        .assert_status(Status::SwitchingProtocols);

    // scheme is not compared, so a tls-terminating proxy doesn't break the check
    handshake(&app)
        .with_request_header("origin", "http://example.com")
        .await
        .assert_status(Status::SwitchingProtocols);

    handshake(&app)
        .with_request_header("origin", "https://evil.example.com")
        .await
        .assert_status(Status::Forbidden);

    handshake(&app)
        .with_request_header("origin", "https://example.com.evil.com")
        .await
        .assert_status(Status::Forbidden);

    // a sandboxed iframe or a file:// page, which is attacker-reachable
    handshake(&app)
        .with_request_header("origin", "null")
        .await
        .assert_status(Status::Forbidden);

    handshake(&app)
        .with_request_header("origin", "not a url")
        .await
        .assert_status(Status::Forbidden);
}

#[test(harness)]
async fn same_origin_ports() {
    let app = app(websocket(|_: WebSocketConn| async {})).await;

    // only this server knows what port it is publicly reachable on, so a port on one side alone
    // is not evidence of a mismatch
    handshake(&app)
        .with_request_header("origin", "https://example.com:8443")
        .await
        .assert_status(Status::SwitchingProtocols);

    // set_host takes a bare host, so the port comes from a base url
    let app = TestServer::new(websocket(|_: WebSocketConn| async {}))
        .await
        .with_base("http://example.com:8443");

    handshake(&app)
        .with_request_header("origin", "https://example.com:8443")
        .await
        .assert_status(Status::SwitchingProtocols);

    handshake(&app)
        .with_request_header("origin", "https://example.com:9999")
        .await
        .assert_status(Status::Forbidden);
}

#[test(harness)]
async fn allow_origins_list() {
    let app = app(websocket(|_: WebSocketConn| async {})
        .allow_origins(["https://app.example.com", "https://admin.example.com:8443"]))
    .await;

    handshake(&app)
        .with_request_header("origin", "https://app.example.com")
        .await
        .assert_status(Status::SwitchingProtocols);

    // default ports normalize
    handshake(&app)
        .with_request_header("origin", "https://app.example.com:443")
        .await
        .assert_status(Status::SwitchingProtocols);

    handshake(&app)
        .with_request_header("origin", "https://admin.example.com:8443")
        .await
        .assert_status(Status::SwitchingProtocols);

    // an explicit list compares the scheme, unlike the same-origin default
    handshake(&app)
        .with_request_header("origin", "http://app.example.com")
        .await
        .assert_status(Status::Forbidden);

    // the origin the server itself is on is not implicitly allowed
    handshake(&app)
        .with_request_header("origin", "https://example.com")
        .await
        .assert_status(Status::Forbidden);

    handshake(&app)
        .with_request_header("origin", "https://app.example.com.evil.com")
        .await
        .assert_status(Status::Forbidden);

    handshake(&app)
        .await
        .assert_status(Status::SwitchingProtocols);
}

#[test(harness)]
async fn allow_origin_fn_distinguishes_absent_from_null() {
    let app =
        app(websocket(|_: WebSocketConn| async {}).allow_origin_fn(|origin| origin.is_none()))
            .await;

    handshake(&app)
        .await
        .assert_status(Status::SwitchingProtocols);

    handshake(&app)
        .with_request_header("origin", "null")
        .await
        .assert_status(Status::Forbidden);

    handshake(&app)
        .with_request_header("origin", "https://example.com")
        .await
        .assert_status(Status::Forbidden);
}

#[test(harness)]
async fn allow_any_origin() {
    let app = app(websocket(|_: WebSocketConn| async {}).allow_any_origin()).await;

    handshake(&app)
        .with_request_header("origin", "https://evil.com")
        .await
        .assert_status(Status::SwitchingProtocols);

    handshake(&app)
        .with_request_header("origin", "null")
        .await
        .assert_status(Status::SwitchingProtocols);
}

#[test]
#[should_panic = "must contain only a scheme, host, and optional port"]
fn allow_origins_rejects_a_path() {
    websocket(|_: WebSocketConn| async {}).allow_origins(["https://example.com/app"]);
}

#[test]
#[should_panic = "could not parse allowed origin"]
fn allow_origins_rejects_a_bare_host() {
    websocket(|_: WebSocketConn| async {}).allow_origins(["example.com"]);
}
