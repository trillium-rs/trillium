use trillium::Method;
use trillium_router::*;
use trillium_testing::{TestServer, harness, test};

#[test(harness)]
async fn disabled_by_default() {
    let app = TestServer::new(Router::new().get("/", "ok")).await;
    // a known path with a mismatched method falls through unchanged (typically a 404)
    app.post("/").await.assert_status(404);
}

#[test(harness)]
async fn method_mismatch_on_known_path_is_405() {
    let app = TestServer::new(
        Router::new()
            .get("/", "ok")
            .post("/", "ok")
            .delete("/other", "ok")
            .with_method_not_allowed(),
    )
    .await;

    app.build(Method::Patch, "/")
        .await
        .assert_status(405)
        .assert_header("allow", "GET, POST");

    // an unknown path has no supported methods, so it still falls through to 404
    app.build(Method::Patch, "/nope").await.assert_status(404);
}

#[test(harness)]
async fn trace_on_known_path_is_405() {
    let app = TestServer::new(Router::new().get("/", "ok").with_method_not_allowed()).await;

    app.build(Method::Trace, "/")
        .await
        .assert_status(405)
        .assert_header("allow", "GET");
}

#[test(harness)]
async fn soft_405_is_replaceable_by_a_later_handler() {
    let app = TestServer::new((
        Router::new().get("/", "ok").with_method_not_allowed(),
        "fallback",
    ))
    .await;

    // a matched route still halts, so the fallback never runs
    app.get("/").await.assert_status(200).assert_body("ok");

    // the 405 is set without halting, so the following handler replaces it
    app.post("/")
        .await
        .assert_status(200)
        .assert_body("fallback");
}
