use super::connection_is_upgrade;
use trillium::Conn;
use trillium_testing::{TestServer, harness, test};

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
