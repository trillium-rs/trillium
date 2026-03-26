# Error handling

Errors in trillium-api are *handlers*. Whether an extraction fails or
your handler returns a `Result::Err`, the error value is run on the conn
just like any other handler. This means error responses are fully
customizable with the same tools you use for success responses.

## Extraction errors

When a [`TryFromConn`](crate::TryFromConn) extractor fails, its
`Error` type is run as a handler on the conn instead of your handler
function. The simplest error type is [`Status`](trillium::Status):

```rust
use trillium::{Conn, Status};
use trillium_api::TryFromConn;
use trillium_router::RouterConnExt;

struct UserId(u64);

impl TryFromConn for UserId {
    type Error = Status;

    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Status> {
        conn.param("user_id")
            .and_then(|p| p.parse().ok())
            .map(UserId)
            .ok_or(Status::BadRequest) // sets 400, no body
    }
}
```

## Built-in `Error` type

[`Body<T>`](crate::Body) and [`Json<T>`](crate::Json) use
[`Error`](crate::Error) as their extraction error type. This type
implements `Handler` with a `before_send` hook that serializes itself
as a JSON error response with an appropriate status code:

- Parse errors → `422 Unprocessable Entity`
- Missing content type → `415 Unsupported Media Type`
- Unsupported content type → `415 Unsupported Media Type`
- I/O errors → `400 Bad Request`

```rust
use trillium_api::{api, Body};
use trillium::Conn;

#[derive(serde::Deserialize)]
struct Input { name: String }

async fn handler(_conn: &mut Conn, Body(input): Body<Input>) -> String {
    format!("hello, {}", input.name)
}

// Sending invalid JSON returns a structured error response:
# use trillium_testing::TestServer;
# use trillium::Status;
# trillium_testing::block_on(async {
#     let app = TestServer::new(api(handler)).await;
#     app.post("/")
#         .with_request_header("content-type", "application/json")
#         .with_body("not json")
#         .await
#         .assert_status(Status::UnprocessableEntity);
# });
// Response body: {"error":{"type":"parse_error","path":".","message":"..."}}
```

## `Result` return types

When your handler returns `Result<T, E>` where both `T` and `E`
implement `Handler`, the result itself is a handler:

```rust
use trillium::{Conn, Handler, Status};
use trillium_api::{api, Json};

#[derive(serde::Serialize)]
struct ApiError { message: String }

/// Implement Handler on your error type to control the error response.
impl Handler for ApiError {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_json(self)
            .with_status(Status::BadRequest)
            .halt()
    }
}

async fn create(_conn: &mut Conn, _: ()) -> Result<Json<String>, ApiError> {
    if true {
        Ok(Json("created".into()))
    } else {
        Err(ApiError { message: "something went wrong".into() })
    }
}
# use trillium_api::ApiConnExt;
# use trillium_testing::TestServer;
# trillium_testing::block_on(async {
#     let app = TestServer::new(api(create)).await;
#     app.get("/").await.assert_ok().assert_body(r#""created""#);
# });
```

## Custom error types

For real applications, you'll typically define an error enum that
covers all your failure modes. The key requirement is that it
implements `Handler`:

```rust
use trillium::{Conn, Handler, Status};
use trillium_api::ApiConnExt;

#[derive(Debug, serde::Serialize, Clone)]
#[serde(tag = "error")]
enum AppError {
    #[serde(rename = "not_found")]
    NotFound { message: String },
    #[serde(rename = "forbidden")]
    Forbidden,
    #[serde(rename = "internal")]
    Internal { message: String },
}

impl Handler for AppError {
    async fn run(&self, conn: Conn) -> Conn {
        let status = match self {
            AppError::NotFound { .. } => Status::NotFound,
            AppError::Forbidden => Status::Forbidden,
            AppError::Internal { .. } => Status::InternalServerError,
        };
        conn.with_json(self).with_status(status).halt()
    }
}
```

You can use this error type as:
- A `TryFromConn::Error` for custom extractors
- The `Err` variant of a `Result` return type

```rust,ignore
impl TryFromConn for Todo {
    type Error = AppError;
    async fn try_from_conn(conn: &mut Conn) -> Result<Self, AppError> {
        // ...
    }
}

async fn update(
    _conn: &mut Conn,
    (todo, Body(input)): (Todo, Body<UpdateTodo>),
) -> Result<Json<Todo>, AppError> {
    // ...
}
```

## Accessing `Error` from `FromConn`

[`Error`](crate::Error) itself implements `FromConn`, extracting (and
removing) any error that a previous handler placed into conn state.
This is useful for custom error formatting in a `before_send` handler:

```rust,ignore
impl Handler for CustomErrorHandler {
    async fn run(&self, conn: Conn) -> Conn { conn }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        if let Some(error) = conn.take_state::<AppError>() {
            // format the error however you like
            conn.with_json(&error).with_status(Status::BadRequest)
        } else {
            conn
        }
    }
}
```
