use serde::{Deserialize, Serialize};
use trillium::{Conn, Status};
use trillium_api::{Handler, TryFromConn, api};
use trillium_testing::{TestResult, TestServer, harness, json, test};

#[derive(Clone, TryFromConn, Handler)]
#[api(state, clone)]
struct CurrentUser {
    name: String,
}

#[derive(Default, Debug)]
struct ServerError;
impl Handler for ServerError {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_status(Status::InternalServerError).halt()
    }
}

#[derive(TryFromConn)]
#[api(state, err = ServerError)]
struct RequiredState(#[allow(dead_code)] u32);

#[derive(Serialize, Deserialize, Handler)]
#[api(json)]
struct Greeting {
    hello: String,
}

#[derive(Serialize, Deserialize, TryFromConn)]
#[api(json)]
struct Submitted {
    name: String,
}

#[derive(Serialize, Deserialize, TryFromConn, Handler)]
#[api(body)]
struct Echo {
    payload: String,
}

#[test(harness)]
async fn state_clone_extracts_self_type_handler_writes_back() -> TestResult {
    let app = TestServer::new((
        |conn: Conn| async move { conn.with_state(CurrentUser { name: "ada".into() }) },
        api(async |_conn: &mut Conn, user: CurrentUser| user),
    ))
    .await;

    app.get("/")
        .await
        .assert_state_with(|CurrentUser { name }| assert_eq!(name, "ada"));

    Ok(())
}

#[test(harness)]
async fn missing_state_runs_default_err_handler() -> TestResult {
    let app = TestServer::new(api(async |_conn: &mut Conn, _: RequiredState| "ok")).await;

    app.get("/")
        .await
        .assert_status(Status::InternalServerError);
    Ok(())
}

#[test(harness)]
async fn json_handler_serializes_self() -> TestResult {
    let app = TestServer::new(api(async |_conn: &mut Conn, _: ()| Greeting {
        hello: "world".into(),
    }))
    .await;

    app.get("/")
        .await
        .assert_ok()
        .assert_header("content-type", "application/json")
        .assert_body(r#"{"hello":"world"}"#);

    Ok(())
}

#[test(harness)]
async fn json_extractor_deserializes_request_body() -> TestResult {
    let app = TestServer::new(api(async |_conn: &mut Conn, s: Submitted| Greeting {
        hello: s.name,
    }))
    .await;

    app.post("/")
        .with_json_body(&json!({"name": "ada"}))
        .await
        .assert_ok()
        .assert_json_body(&json!({"hello": "ada"}));
    Ok(())
}

#[test(harness)]
async fn body_roundtrips_via_content_negotiation() -> TestResult {
    let app = TestServer::new(api(async |_conn: &mut Conn, e: Echo| e)).await;

    app.post("/")
        .with_request_header("accept", "application/json")
        .with_json_body(&json!({"payload": "hi"}))
        .await
        .assert_ok()
        .assert_header("content-type", "application/json")
        .assert_body(r#"{"payload":"hi"}"#);

    app.post("/")
        .with_request_header("accept", "application/x-www-form-urlencoded")
        .with_request_header("content-type", "application/x-www-form-urlencoded")
        .with_body("payload=hi")
        .await
        .assert_ok()
        .assert_header("content-type", "application/x-www-form-urlencoded")
        .assert_body("payload=hi");

    app.post("/")
        .with_request_header("accept", "application/json")
        .with_request_header("content-type", "application/x-www-form-urlencoded")
        .with_body("payload=hi")
        .await
        .assert_ok()
        .assert_header("content-type", "application/json")
        .assert_body(r#"{"payload":"hi"}"#);

    Ok(())
}
