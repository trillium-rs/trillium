use serde::{Deserialize, Serialize};
use trillium::{Conn, Handler, Headers, KnownHeaderName, Status};
use trillium_api::*;
use trillium_testing::{TestServer, harness, test};

#[derive(Serialize, Deserialize, Debug)]
struct Struct {
    string: String,
    numbers: Option<Vec<usize>>,
}

#[derive(Serialize, Deserialize, Debug)]
struct ApiResponse {
    s: Struct,
}

fn app_with_body() -> impl Handler {
    api(|__: &mut Conn, Body(mut s): Body<Struct>| async move {
        if let Some(numbers) = &mut s.numbers {
            numbers.push(100);
        }
        Body(ApiResponse { s })
    })
}

#[test(harness)]
async fn json_request_json_response() {
    let app = TestServer::new(app_with_body()).await;

    app.post("/")
        .with_request_header("content-type", "application/json")
        .with_body(r#"{"string": "string", "numbers": [ 1, 2, 3]}"#)
        .await
        .assert_ok()
        .assert_body(r#"{"s":{"string":"string","numbers":[1,2,3,100]}}"#);
}

#[test(harness)]
async fn form_urlencoded_json_response() {
    let app = TestServer::new(app_with_body()).await;

    app.post("/")
        .with_request_header("content-type", "application/x-www-form-urlencoded")
        .with_body(r#"string=string"#)
        .await
        .assert_ok()
        .assert_body(r#"{"s":{"string":"string","numbers":null}}"#);
}

#[cfg(feature = "sonic-rs")]
#[test(harness)]
async fn malformed_json_request() {
    let app = TestServer::new(app_with_body()).await;

    let response = app
        .post("/")
        .with_request_header("content-type", "application/json")
        .with_body(r#"this is not valid json"#)
        .await;

    response.assert_status(422);
    let response_body = response.body();
    let expected = sonic_rs::json!({"error": {"path": ".", "message": "Invalid literal (`true`, `false`, or a `null`) while parsing at line 1 column 4\n\n\tthis is not\n\t...^.......\n","type":"parse_error"}});
    assert_eq!(
        sonic_rs::from_str::<sonic_rs::Value>(response_body).unwrap(),
        expected
    );
}

fn app_without_body() -> impl Handler {
    api(|_: &mut Conn, _: ()| async { Json(json!({"health": "ok" })) })
}

#[test(harness)]
async fn get_json_response() {
    let app = TestServer::new(app_without_body()).await;

    app.get("/")
        .await
        .assert_ok()
        .assert_body(r#"{"health":"ok"}"#)
        .assert_header(KnownHeaderName::ContentType, "application/json");
}

#[test(harness)]
async fn get_custom_content_type() {
    let handler = (
        Headers::from_iter([(KnownHeaderName::ContentType, "application/custom+json")]),
        Json(json!({"health": "ok"})),
    );
    let app = TestServer::new(handler).await;

    app.get("/")
        .await
        .assert_ok()
        .assert_body(r#"{"health":"ok"}"#)
        .assert_header(KnownHeaderName::ContentType, "application/custom+json");
}

fn app_with_json() -> impl Handler {
    api(|_: &mut Conn, Json(value): Json<Value>| async { Json(value) })
}

#[test(harness)]
async fn json_try_from_conn_checks_content_type() {
    let app = TestServer::new(app_with_json()).await;

    app.post("/")
        .with_request_header("content-type", "application/x-www-form-urlencoded")
        .with_body(r#"string=string"#)
        .await
        .assert_status(trillium::Status::UnsupportedMediaType);

    app.post("/")
        .with_request_header("content-type", "application/json")
        .with_body(r#"{"string": 1}"#)
        .await
        .assert_ok();
}

async fn error_handler(conn: &mut Conn, error: Error) {
    conn.set_body(format!("my error format: {error:?}"));
    conn.set_status(&error);
}

fn app_with_error_handler() -> impl Handler {
    (
        api(|_: &mut Conn, Json(value): Json<Value>| async { Json(value) }),
        BeforeSend(api(error_handler)),
    )
}

#[test(harness)]
async fn error_handler_works() {
    let _ = env_logger::builder().is_test(true).try_init();
    let app = TestServer::new(app_with_error_handler()).await;

    app.post("/")
        .with_request_header("content-type", "application/x-www-form-urlencoded")
        .with_body(r#"string=string"#)
        .await
        .assert_status(Status::UnsupportedMediaType)
        .assert_body(
            "my error format: UnsupportedMimeType { mime_type: \
             \"application/x-www-form-urlencoded\" }",
        );
}
