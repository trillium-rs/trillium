# API Layer

[rustdocs](https://docs.trillium.rs/trillium_api)

The `trillium-api` crate provides an extractor-based handler interface for building typed APIs. Instead of reading from `Conn` by hand, you write async functions that declare what data they need and the framework extracts it automatically.

## Basic usage

Wrap an async function with `api()` to turn it into a `Handler`:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-api = { path = "../api", features = ["sonic-rs"] }
#
use trillium::Conn;
use trillium_api::{Json, api};

async fn hello(_conn: &mut Conn, _: ()) -> Json<&'static str> {
    Json("hello, world")
}

fn main() {
    trillium_smol::run(api(hello));
}
```

The function signature drives behavior:
- The first parameter is always `&mut Conn`.
- The second parameter is extracted from the request using `TryFromConn` or `FromConn`.
- The return value is run as a `Handler` on the conn — return a status, a `Json<T>`, a string, or anything else that implements `Handler`.

## Extractors

| Type | Extracts | Fails if |
|------|----------|---------|
| `()` | Nothing | Never |
| `Json<T>` | Deserializes JSON request body into `T` | Body is not valid JSON, or `T` fails to deserialize |
| `Body<T>` | Deserializes body based on `Content-Type` | Unsupported content type, or deserialization fails |
| `State<T>` | Takes `T` from conn state | State is absent (halts with no body) |
| `String` | Request body as a string | Body is not valid UTF-8 |
| `Vec<u8>` | Request body as raw bytes | Never |
| `Method` | The HTTP method | Never |
| `Headers` | Clone of request headers | Never |
| `(A, B, ...)` | Multiple extractors as a tuple | If any constituent fails |

## JSON request and response

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-api = { path = "../api" }
# serde = { version = "*", features = ["derive"] }
#
use serde::{Deserialize, Serialize};
use trillium_api::Json;
use trillium::Conn;

#[derive(Deserialize)]
struct CreatePost { title: String, body: String }

#[derive(Serialize)]
struct Post { id: u64, title: String, body: String }

async fn create_post(_conn: &mut Conn, Json(input): Json<CreatePost>) -> Json<Post> {
    // In a real app you'd persist this somewhere
    Json(Post { id: 1, title: input.title, body: input.body })
}
# fn main() {}
```

## Error handling

Errors in `trillium-api` are handlers. When an extraction fails, the extractor's error type is run on the conn instead of your function. When a handler returns `Result<T, E>` and the result is `Err(e)`, `e` is run as a handler.

### Extraction errors

`Json<T>` and `Body<T>` use `trillium_api::Error` as their error type, which responds with a structured JSON error body and an appropriate status:

- Parse errors → `422 Unprocessable Entity`
- Missing content type → `415 Unsupported Media Type`
- I/O errors → `400 Bad Request`

These are handled automatically — if a client sends malformed JSON to a handler that extracts `Json<T>`, the response is a structured error without your function being called.

### Result return types

Your handler can return `Result<T, E>` where both `T` and `E` implement `Handler`. The idiomatic pattern is to define an error type for your application:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-api = { path = "../api", features = ["sonic-rs"] }
# serde = { version = "*", features = ["derive"] }
#
use trillium::{Conn, Handler, Status};
use trillium_api::{Json, ApiConnExt};
use serde::Serialize;

#[derive(Serialize)]
struct ApiError { message: String }

impl Handler for ApiError {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_json(self)
            .with_status(Status::BadRequest)
            .halt()
    }
}

async fn divide(_conn: &mut Conn, Json((a, b)): Json<(f64, f64)>) -> Result<Json<f64>, ApiError> {
    if b == 0.0 {
        Err(ApiError { message: "division by zero".into() })
    } else {
        Ok(Json(a / b))
    }
}
# fn main() {}
```

For extraction errors specifically, `Status` alone is a valid and simple error type — it sets the status code when run as a handler:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-api = { path = "../api" }
# trillium-router = { path = "../router" }
#
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
            .ok_or(Status::BadRequest)
    }
}
# fn main() {}
```

## Combining with the router

`api()` returns a `Handler`, so it composes naturally with the router:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-router = { path = "../router" }
# trillium-api = { path = "../api" }
#
# use trillium::Conn;
# async fn list_posts(conn: &mut Conn, _: ()) { conn; }
# async fn create_post(conn: &mut Conn, _: ()) { conn; }
# async fn get_post(conn: &mut Conn, _: ()) { conn; }
# fn main() {
use trillium_router::router;
use trillium_api::api;

let app = router()
    .get("/posts", api(list_posts))
    .post("/posts", api(create_post))
    .get("/posts/:id", api(get_post));
# trillium_smol::run(app);
# }
```

## JSON serialization backend

trillium-api does not enable any default features, but you likely want to select either `serde_json`
or `sonic-rs` to get the most out of this crate. The two features are mutually exclusive.

See the [rustdocs](https://docs.trillium.rs/trillium_api) for the full extractor API, custom
extractor implementation, and return type details.
