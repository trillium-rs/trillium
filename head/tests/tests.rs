use trillium::{Conn, Method};
use trillium_head::*;
use trillium_testing::{TestServer, harness, test};

#[test(harness)]
async fn test() {
    let app = TestServer::new((Head::new(), |conn: Conn| async move {
        match (conn.method(), conn.path()) {
            (Method::Get, "/") => conn.ok("ok, this is my body"),
            (Method::Get, _) => conn.with_status(404).with_body("egads i don't have that"),
            _ => conn,
        }
    }))
    .await;

    app.build(Method::Head, "/")
        .await
        .assert_ok()
        .assert_body("")
        .assert_header("content-length", "19");

    app.get("/")
        .await
        .assert_ok()
        .assert_body("ok, this is my body")
        .assert_header("content-length", "19");

    app.build(Method::Head, "/not_found")
        .await
        .assert_status(404)
        .assert_body("")
        .assert_header("content-length", "23");

    app.get("/not_found")
        .await
        .assert_status(404)
        .assert_body("egads i don't have that")
        .assert_header("content-length", "23");

    app.post("/").await.assert_status(404);
}
