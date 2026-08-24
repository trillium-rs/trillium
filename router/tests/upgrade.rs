use trillium::{Conn, Handler, Upgrade};
use trillium_router::Router;
use trillium_testing::{TestServer, futures_lite::AsyncWriteExt, harness, test};

struct UpgradeProbe;
impl Handler for UpgradeProbe {
    async fn run(&self, conn: Conn) -> Conn {
        conn.upgrade().with_status(200).halt()
    }

    fn has_upgrade(&self, _upgrade: &Upgrade) -> bool {
        true
    }

    async fn upgrade(&self, mut upgrade: Upgrade) {
        upgrade.write_all(b"i was an upgrade").await.unwrap();
        upgrade.close().await.unwrap();
    }
}

#[test(harness)]
async fn flat_router_dispatches_upgrade() {
    let app = TestServer::new(Router::new().get("/hello", UpgradeProbe)).await;

    app.get("/hello")
        .await
        .assert_ok()
        .assert_body("i was an upgrade");
}

#[test(harness)]
async fn nested_router_dispatches_upgrade() {
    let app =
        TestServer::new(Router::new().all("/api/*", Router::new().get("/hello", UpgradeProbe)))
            .await;

    app.get("/api/hello")
        .await
        .assert_ok()
        .assert_body("i was an upgrade");
}

#[test(harness)]
async fn doubly_nested_router_dispatches_upgrade() {
    let app = TestServer::new(Router::new().all(
        "/api/*",
        Router::new().all("/v1/*", Router::new().get("/hello", UpgradeProbe)),
    ))
    .await;

    app.get("/api/v1/hello")
        .await
        .assert_ok()
        .assert_body("i was an upgrade");
}
