use trillium::{Conn, Handler};
use trillium_router::Router;
use trillium_testing::{TestServer, harness, test};

struct Probe;
impl Handler for Probe {
    async fn run(&self, conn: Conn) -> Conn {
        conn.ok("run-ok")
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        conn.with_response_header("x-probe", "before-send-ran")
    }
}

#[test(harness)]
async fn flat_router_runs_before_send() {
    let app = TestServer::new(Router::new().get("/hello", Probe)).await;

    app.get("/hello")
        .await
        .assert_ok()
        .assert_body("run-ok")
        .assert_header("x-probe", "before-send-ran");
}

#[test(harness)]
async fn nested_router_runs_before_send() {
    let app =
        TestServer::new(Router::new().all("/api/*", Router::new().get("/hello", Probe))).await;

    app.get("/api/hello")
        .await
        .assert_ok()
        .assert_body("run-ok")
        .assert_header("x-probe", "before-send-ran");
}

#[test(harness)]
async fn doubly_nested_router_runs_before_send() {
    let app = TestServer::new(Router::new().all(
        "/api/*",
        Router::new().all("/v1/*", Router::new().get("/hello", Probe)),
    ))
    .await;

    app.get("/api/v1/hello")
        .await
        .assert_ok()
        .assert_body("run-ok")
        .assert_header("x-probe", "before-send-ran");
}
