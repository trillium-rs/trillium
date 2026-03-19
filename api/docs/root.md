An extractor-based API layer for trillium.

This crate provides [`api`], which wraps an async function into a trillium
[`Handler`](trillium::Handler). The function receives a `&mut Conn` and an *extracted* value
(deserialized body, shared state, route parameters, etc.) and returns
any type that implements `Handler`.

```rust
use trillium_api::{api, Json};
use trillium::Conn;

/// An api handler that takes no input and returns a JSON response.
async fn hello(_conn: &mut Conn, _: ()) -> Json<&'static str> {
    Json("hello, world")
}

/// An api handler that deserializes a JSON body and echoes it back.
async fn echo(_conn: &mut Conn, Json(body): Json<trillium_api::Value>) -> Json<trillium_api::Value> {
    Json(body)
}

# use trillium_testing::TestHandler;
# trillium_testing::block_on(async {
#     let app = TestHandler::new(api(hello)).await;
#     app.get("/").await.assert_ok().assert_body(r#""hello, world""#);
#
#     let app = TestHandler::new(api(echo)).await;
#     app.post("/")
#         .with_body(r#"{"key":"value"}"#)
#         .with_request_header("content-type", "application/json")
#         .await
#         .assert_ok()
#         .assert_body(r#"{"key":"value"}"#);
# });
```

## How it works

When a request arrives, [`api`] does three things:

1. **Extract** — calls [`TryFromConn`] on the second parameter to pull
   typed data out of the conn (body, state, headers, etc.)
2. **Call** — passes `&mut Conn` and the extracted value to your function
3. **Run** — takes whatever your function returned (which must implement
   [`Handler`](trillium::Handler)) and runs it on the conn

If extraction fails, your function is never called. Instead, the
[`TryFromConn::Error`] type — which must itself implement `Handler` — is
run on the conn, typically setting an error status.

## Guide

| Module | Topic |
|--------|-------|
| [`extractors`] | Pulling data out of requests — `Body`, `Json`, `State`, tuples |
| [`extractors::custom`] | Writing your own `FromConn` / `TryFromConn` implementations |
| [`return_types`] | What you can return from an api handler |
| [`error_handling`] | How extraction errors and `Result` return types work |
| [`recipes`] | Patterns and ideas: middleware, type aliases, and more |

## Extractors at a glance

| Type | Extracts | Fallible? |
|------|----------|-----------|
| `()` | Nothing (no-op) | No |
| [`Body<T>`] | Deserialized request body (content-type negotiated) | Yes |
| [`Json<T>`] | Deserialized request body (JSON only) | Yes |
| [`State<T>`] | A `T` from conn state (via [`take_state`](trillium::Conn::take_state)) | No* |
| `String` | Request body as a string | Yes |
| `Vec<u8>` | Request body as raw bytes | Yes |
| `Headers` | Clone of request headers | No |
| `Method` | The HTTP method | No |
| `(A, B, ...)` | Multiple extractors as a tuple (up to 12) | Depends |

*`State<T>` returns `None` (halting the conn) if the state is missing.

## Formats supported

This crate supports *receiving* `application/json` and
`application/x-www-form-urlencoded` by default. To disable form support,
use `default-features = false`. Response serialization uses
`Accept` header negotiation.

JSON serialization defaults to `sonic-rs`. To use `serde_json`
instead, set `default-features = false, features = ["serde_json"]`.
The two features are mutually exclusive.
