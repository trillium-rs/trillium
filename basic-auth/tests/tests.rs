use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use trillium::Conn;
use trillium_basic_auth::{BasicAuth, BasicAuthConnExt, Credentials};
use trillium_testing::{TestServer, harness, test};

fn auth_header(username: &str, password: &str) -> String {
    format!("Basic {}", BASE64.encode(format!("{username}:{password}")))
}

async fn report_username(conn: Conn) -> Conn {
    let username = conn.basic_auth_username().unwrap_or_default().to_string();
    conn.ok(username)
}

#[test(harness)]
async fn correct_auth() {
    let app = TestServer::new((BasicAuth::new("jacob", "7r1ll1um"), report_username)).await;

    app.get("/")
        .with_request_header("Authorization", auth_header("jacob", "7r1ll1um"))
        .await
        .assert_ok()
        .assert_body("jacob");

    // RFC 9110 auth-scheme matching is case-insensitive
    app.get("/")
        .with_request_header(
            "Authorization",
            auth_header("jacob", "7r1ll1um").replace("Basic", "basic"),
        )
        .await
        .assert_ok()
        .assert_body("jacob");
}

#[test(harness)]
async fn incorrect_auth() {
    let app = TestServer::new((BasicAuth::new("jacob", "7r1ll1um"), "ok")).await;

    app.get("/")
        .with_request_header("Authorization", auth_header("jacob", "wrong"))
        .await
        .assert_status(401)
        .assert_header("www-authenticate", "Basic");

    app.get("/")
        .with_request_header("Authorization", auth_header("jacob:7r1ll1um", ""))
        .await
        .assert_status(401);

    app.get("/")
        .with_request_header("Authorization", "Basic not-base64!")
        .await
        .assert_status(401);

    app.get("/")
        .with_request_header("Authorization", "Bearer some-token")
        .await
        .assert_status(401);

    app.get("/").await.assert_status(401);
}

#[test(harness)]
async fn incorrect_auth_with_realm() {
    let app = TestServer::new((
        BasicAuth::new("gunter", "quack").with_realm("kingdom of ooo"),
        "ok",
    ))
    .await;

    app.get("/")
        .with_request_header("Authorization", auth_header("orgalorg", "31337"))
        .await
        .assert_status(401)
        .assert_header("www-authenticate", "Basic realm=\"kingdom of ooo\"");
}

#[test(harness)]
async fn reuses_handler_across_requests() {
    let app = TestServer::new((BasicAuth::new("jacob", "7r1ll1um"), report_username)).await;

    app.get("/")
        .with_request_header("Authorization", auth_header("jacob", "7r1ll1um"))
        .await
        .assert_ok()
        .assert_body("jacob");

    app.get("/")
        .with_request_header("Authorization", auth_header("jacob", "wrong"))
        .await
        .assert_status(401);
}

#[test(harness)]
async fn predicate() {
    let app = TestServer::new((
        BasicAuth::validate_fn(|credentials| credentials.password() == "open sesame"),
        report_username,
    ))
    .await;

    app.get("/")
        .with_request_header("Authorization", auth_header("ali baba", "open sesame"))
        .await
        .assert_ok()
        .assert_body("ali baba");

    app.get("/")
        .with_request_header("Authorization", auth_header("ali baba", "open barley"))
        .await
        .assert_status(401);
}

#[test(harness)]
async fn async_predicate() {
    let app = TestServer::new((
        BasicAuth::validate_async_fn(|credentials: Credentials| async move {
            credentials.username().starts_with("admin-")
        }),
        report_username,
    ))
    .await;

    app.get("/")
        .with_request_header("Authorization", auth_header("admin-jacob", "whatever"))
        .await
        .assert_ok()
        .assert_body("admin-jacob");

    app.get("/")
        .with_request_header("Authorization", auth_header("jacob", "whatever"))
        .await
        .assert_status(401);
}

#[test(harness)]
async fn credentials_from_conn() {
    let app = TestServer::new((BasicAuth::validate_fn(|_| true), |conn: Conn| async move {
        let credentials = Credentials::from_conn(&conn).unwrap();
        let body = format!("{}/{}", credentials.username(), credentials.password());
        conn.ok(body)
    }))
    .await;

    // the api-key-as-password convention: a colon in the password is preserved
    app.get("/")
        .with_request_header("Authorization", auth_header("x", "key:with:colons"))
        .await
        .assert_body("x/key:with:colons");
}

#[test]
fn credentials_debug_masks_the_password() {
    let debug = format!("{:?}", Credentials::new("jacob", "7r1ll1um"));
    assert!(!debug.contains("7r1ll1um"), "{debug}");
    assert!(debug.contains("jacob"), "{debug}");

    let debug = format!("{:?}", BasicAuth::new("jacob", "7r1ll1um"));
    assert!(!debug.contains("7r1ll1um"), "{debug}");
}
