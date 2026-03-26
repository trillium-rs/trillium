use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use trillium_basic_auth::{BasicAuth, Credentials};
use trillium_testing::{TestServer, harness, test};

fn auth_header(username: &str, password: &str) -> String {
    format!("Basic {}", BASE64.encode(format!("{username}:{password}")))
}

#[test(harness)]
async fn correct_auth() {
    let app = TestServer::new((BasicAuth::new("jacob", "7r1ll1um"), "ok")).await;

    app.get("/")
        .with_request_header("Authorization", auth_header("jacob", "7r1ll1um"))
        .await
        .assert_ok()
        .assert_body("ok")
        .assert_state_with(|credentials: &Credentials| {
            assert_eq!(credentials.username(), "jacob");
            assert_eq!(credentials.password(), "7r1ll1um");
        });
}

#[test(harness)]
async fn incorrect_auth() {
    let app = TestServer::new((BasicAuth::new("jacob", "7r1ll1um"), "ok")).await;

    app.get("/")
        .with_request_header("Authorization", auth_header("jacob", "wrong"))
        .await
        .assert_status(401)
        .assert_header("www-authenticate", "Basic")
        .assert_no_state::<Credentials>();
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
        .assert_header("www-authenticate", "Basic realm=\"kingdom of ooo\"")
        .assert_no_state::<Credentials>();
}

#[test(harness)]
async fn reuses_handler_across_requests() {
    let app = TestServer::new((BasicAuth::new("jacob", "7r1ll1um"), "ok")).await;

    app.get("/")
        .with_request_header("Authorization", auth_header("jacob", "7r1ll1um"))
        .await
        .assert_ok()
        .assert_state_with(|credentials: &Credentials| {
            assert_eq!(credentials.username(), "jacob");
            assert_eq!(credentials.password(), "7r1ll1um");
        });

    app.get("/")
        .with_request_header("Authorization", auth_header("jacob", "wrong"))
        .await
        .assert_status(401)
        .assert_no_state::<Credentials>();
}
