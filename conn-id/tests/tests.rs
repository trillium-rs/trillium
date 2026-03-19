use std::sync::atomic::{AtomicUsize, Ordering};
use trillium::Conn;
use trillium_conn_id::*;
use trillium_testing::{TestHandler, harness, test};
use uuid::Uuid;

fn build_incrementing_id_generator() -> impl Fn() -> String + Send + Sync + 'static {
    let count = AtomicUsize::default();
    move || count.fetch_add(1, Ordering::SeqCst).to_string()
}

#[derive(Debug, PartialEq, Eq)]
struct Id(String);
async fn set_id_state_for_tests(conn: Conn) -> Conn {
    let id = Id(conn.id().to_string());
    conn.with_state(id)
}

#[test(harness)]
async fn test_defaults() {
    let app = TestHandler::new((ConnId::new().with_seed(1000), set_id_state_for_tests, "ok")).await;

    app.get("/")
        .await
        .assert_ok()
        .assert_body("ok")
        .assert_header("x-request-id", "J4lzoPXcT5");

    app.get("/")
        .await
        .assert_ok()
        .assert_body("ok")
        .assert_header("x-request-id", "Sn0wUTe4EF");

    app.get("/")
        .with_request_header("x-request-id", "inbound-id")
        .await
        .assert_ok()
        .assert_body("ok")
        .assert_header("x-request-id", "inbound-id");

    app.get("/")
        .await
        .assert_ok()
        .assert_state(Id("cnx2OnqZsR".to_string()));
}

#[test(harness)]
async fn test_settings() {
    let app = TestHandler::new((
        ConnId::new()
            .with_request_header("x-custom-id")
            .with_response_header("x-something-else")
            .with_id_generator(|| Uuid::new_v4().to_string()),
        set_id_state_for_tests,
        "ok",
    ))
    .await;

    app.get("/")
        .await
        .assert_ok()
        .assert_header_with("x-something-else", |header| {
            assert!(Uuid::parse_str(header.as_str().unwrap()).is_ok());
        })
        .assert_state_with(|state_id: &Id| {
            assert!(Uuid::parse_str(&state_id.0).is_ok());
        });

    app.get("/")
        .with_request_header("x-custom-id", "inbound-id")
        .await
        .assert_ok()
        .assert_body("ok")
        .assert_header("x-something-else", "inbound-id");
}

#[test(harness)]
async fn test_no_headers() {
    let app = TestHandler::new((
        ConnId::new()
            .with_id_generator(build_incrementing_id_generator())
            .without_request_header()
            .without_response_header(),
        set_id_state_for_tests,
        "ok",
    ))
    .await;

    app.get("/")
        .await
        .assert_ok()
        .assert_no_header("x-request-id")
        .assert_state(Id("0".to_string()));

    app.get("/")
        .with_request_header("x-request-id", "ignored")
        .await
        .assert_ok()
        .assert_no_header("x-request-id")
        .assert_state(Id("1".to_string()));
}
