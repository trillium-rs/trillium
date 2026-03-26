use trillium::{Conn, Method};
use trillium_method_override::*;
use trillium_testing::{TestServer, harness, test};

async fn test_handler(conn: Conn) -> Conn {
    match (conn.method(), conn.path()) {
        (Method::Delete, _) => conn.ok("you did a delete"),
        (Method::Post, _) => conn.ok("it was a post"),
        (Method::Patch, _) => conn.ok("adams"),
        (Method::Put, _) => conn.ok("put and call"),
        _ => conn,
    }
}

#[test(harness)]
async fn test() {
    let app = TestServer::new((MethodOverride::new(), test_handler)).await;

    app.post("/?_method=delete")
        .await
        .assert_ok()
        .assert_body("you did a delete");

    app.post("/?a=b&_method=delete&c=d")
        .await
        .assert_ok()
        .assert_body("you did a delete");

    app.post("/?_method=connect")
        .await
        .assert_ok()
        .assert_body("it was a post");

    app.post("/?_method!!-=/=connect")
        .await
        .assert_ok()
        .assert_body("it was a post");

    app.get("/?_method=delete").await.assert_status(404);
}

#[test(harness)]
async fn with_limited_allowed_methods() {
    let app = TestServer::new((
        MethodOverride::new().with_allowed_methods(["put", "patch"]),
        test_handler,
    ))
    .await;

    app.post("/?_method=put")
        .await
        .assert_ok()
        .assert_body("put and call");

    app.post("/?a=b&_method=patch&c=d")
        .await
        .assert_ok()
        .assert_body("adams");

    app.post("/?_method=delete")
        .await
        .assert_ok()
        .assert_body("it was a post");
}

#[test(harness)]
async fn with_a_different_param_name() {
    let app = TestServer::new((MethodOverride::new().with_param_name("verb"), test_handler)).await;

    app.post("/?verb=delete")
        .await
        .assert_ok()
        .assert_body("you did a delete");

    app.post("/?_method=delete")
        .await
        .assert_ok()
        .assert_body("it was a post");
}
