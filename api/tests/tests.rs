use serde::{Deserialize, Serialize};
use trillium_api::*;
use trillium_testing::prelude::*;

#[derive(Serialize, Deserialize, Debug)]
struct Struct {
    string: String,
    numbers: Option<Vec<usize>>,
}

#[derive(Serialize, Deserialize, Debug)]
struct ApiResponse {
    s: Struct,
}

fn app() -> impl trillium::Handler {
    api(|conn: trillium::Conn, mut s: Struct| async move {
        if let Some(numbers) = &mut s.numbers {
            numbers.push(100);
        }
        conn.with_json(&ApiResponse { s })
    })
}

#[test]
fn json_request_json_response() {
    assert_ok!(
        get("/")
            .with_request_header("content-type", "application/json")
            .with_request_body(r#"{"string": "string", "numbers": [ 1, 2, 3]}"#)
            .on(&app()),
        r#"{"s":{"string":"string","numbers":[1,2,3,100]}}"#
    );
}

#[test]
fn form_urlencoded_json_response() {
    assert_ok!(
        get("/")
            .with_request_header("content-type", "application/x-www-form-urlencoded")
            .with_request_body(r#"string=string"#)
            .on(&app()),
        r#"{"s":{"string":"string","numbers":null}}"#
    );
}
