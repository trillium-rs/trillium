use trillium::Method;
use trillium_router::*;
use trillium_testing::{TestServer, harness, test};

#[test(harness)]
async fn options_star_with_a_star_handler() {
    let app = TestServer::new(
        Router::new()
            .get("*", "ok")
            .post("/some/specific/route", "ok"),
    )
    .await;

    app.build(Method::Options, "*")
        .await
        .assert_status(200)
        .assert_header("allow", "GET, POST");
}

#[test(harness)]
async fn options_specific_route_with_several_matching_methods() {
    let app = TestServer::new(
        Router::new()
            .get("*", "ok")
            .post("/some/specific/route", "ok")
            .delete("/some/specific/:anything", "ok"),
    )
    .await;

    app.build(Method::Options, "/some/specific/route")
        .await
        .assert_status(200)
        .assert_header("allow", "DELETE, GET, POST");

    app.build(Method::Options, "/some/specific/other")
        .await
        .assert_status(200)
        .assert_header("allow", "DELETE, GET");

    app.build(Method::Options, "/only-get")
        .await
        .assert_status(200)
        .assert_header("allow", "GET");
}

#[test(harness)]
async fn options_specific_route_with_no_matching_routes() {
    let app = TestServer::new(
        Router::new()
            .post("/some/specific/route", "ok")
            .delete("/some/specific/:anything", "ok"),
    )
    .await;

    app.build(Method::Options, "/other")
        .await
        .assert_status(200)
        .assert_header("allow", "");
}

#[test(harness)]
async fn options_any() {
    let app =
        TestServer::new(Router::new().any(&["delete", "get", "patch"], "/some-route", "ok")).await;

    app.build(Method::Options, "*")
        .await
        .assert_status(200)
        .assert_header("allow", "DELETE, GET, PATCH");
}

#[test(harness)]
async fn when_options_are_disabled() {
    let app = TestServer::new(Router::new().without_options_handling().get("*", "ok")).await;

    app.build(Method::Options, "/").await.assert_status(404);
}

#[test(harness)]
async fn nested_router() {
    let app = TestServer::new(Router::new().all(
        "/nested/*",
        Router::new().get("/here", "ok").post("*", "ok"),
    ))
    .await;

    app.build(Method::Options, "/nested/here")
        .await
        .assert_status(200)
        .assert_header("allow", "GET, POST");

    app.build(Method::Options, "*")
        .await
        .assert_status(200)
        .assert_header("allow", "DELETE, GET, PATCH, POST, PUT");
}
