# Extractors — pulling data out of Conns

The second parameter of an [`api`](crate::api) handler is the *extractor*
— a type that implements [`TryFromConn`](crate::TryFromConn) (or its
infallible cousin [`FromConn`](crate::FromConn)). Before your handler
function runs, the extractor pulls typed data out of the
[`Conn`](trillium::Conn).

The recommended way to make your own types into extractors is the
[`#[derive(TryFromConn)]`](crate::TryFromConn) macro — see
[`extractors::deriving`](crate::extractors::deriving) for the full
reference.

## Deriving extractors on your own types

If your type is already a regular Rust struct (often a `serde::Deserialize`
domain type or a clonable handle to shared state), the simplest path is to
let the derive write the `TryFromConn` impl for you:

```rust
use trillium::Conn;
use trillium_api::{api, TryFromConn};
use serde::Deserialize;

#[derive(Deserialize, TryFromConn)]
#[api(json)]
struct NewPost {
    title: String,
}

async fn create(_conn: &mut Conn, input: NewPost) -> String {
    format!("created: {}", input.title)
}
# use trillium_testing::TestServer;
# trillium_testing::block_on(async {
#     let app = TestServer::new(api(create)).await;
#     app.post("/")
#         .with_request_header("content-type", "application/json")
#         .with_body(r#"{"title":"hello"}"#)
#         .await
#         .assert_ok()
#         .assert_body("created: hello");
# });
```

The `#[api(...)]` attribute selects how the type is extracted:

- `#[api(json)]` — JSON request body
- `#[api(body)]` — request body with content-type negotiation
- `#[api(state)]` — pulled out of conn state via
  [`take_state`](trillium::Conn::take_state)
- `#[api(state, clone)]` — same, but cloning rather than taking
- `#[api(state, err = MyHandler)]` — run `MyHandler::default()` as a handler
  if the state is missing

The companion [`#[derive(Handler)]`](trillium::Handler) macro generates a
`Handler` impl symmetrically — `#[api(json)]` writes JSON, `#[api(state)]`
writes into state, `#[api(body)]` serializes with content-type negotiation:

```rust
use trillium_api::{api, Handler, TryFromConn};
use trillium::Conn;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, TryFromConn, Handler)]
#[api(body)]
struct Echo {
    payload: String,
}

/// Reads `Echo` from the request body, returns the same value as the response.
async fn echo(_conn: &mut Conn, e: Echo) -> Echo {
    e
}
# use trillium_testing::TestServer;
# trillium_testing::block_on(async {
#     let app = TestServer::new(api(echo)).await;
#     app.post("/")
#         .with_request_header("content-type", "application/json")
#         .with_request_header("accept", "application/json")
#         .with_body(r#"{"payload":"hi"}"#)
#         .await
#         .assert_ok()
#         .assert_body(r#"{"payload":"hi"}"#);
# });
```

See [`extractors::deriving`](crate::extractors::deriving) for the full set
of options and the generated code shape.

## No extraction

Use `()` when you don't need anything from the request:

```rust
use trillium_api::api;
use trillium::Conn;

async fn health(_conn: &mut Conn, _: ()) -> &'static str {
    "ok"
}
# use trillium_testing::TestServer;
# trillium_testing::block_on(async {
#     let app = TestServer::new(api(health)).await;
#     app.get("/").await.assert_ok().assert_body("ok");
# });
```

## Built-in request metadata

Some trillium types implement [`FromConn`](crate::FromConn) directly:

```rust
use trillium_api::api;
use trillium::{Conn, Headers, Method};

async fn inspect(_conn: &mut Conn, (method, headers): (Method, Headers)) -> String {
    format!("{} with {} headers", method, headers.len())
}
# use trillium_testing::TestServer;
# trillium_testing::block_on(async {
#     let app = TestServer::new(api(inspect)).await;
#     app.get("/").await.assert_ok();
# });
```

You can also extract the body as a raw `String` or `Vec<u8>`:

```rust
use trillium_api::api;
use trillium::Conn;

async fn raw_body(_conn: &mut Conn, body: String) {
    // `body` is the request body as a string
}
```

## Tuple extraction

Combine multiple extractors as a tuple (up to 12 elements). Extractors
run in order, left to right. If any one fails, the error handler for
that extractor runs and subsequent extractors are skipped.

```rust
use trillium_api::{api, Handler, TryFromConn, Json};
use trillium::{Conn, Status};
use serde::{Deserialize, Serialize};

#[derive(Clone, TryFromConn)]
#[api(state, clone)]
struct Db;

#[derive(Deserialize, TryFromConn)]
#[api(json)]
struct CreateItem { name: String }

#[derive(Serialize)]
struct Item { id: u64, name: String }

async fn create(
    _conn: &mut Conn,
    (db, input): (Db, CreateItem),
) -> (Status, Json<Item>) {
    let _ = db; // use the database...
    (Status::Created, Json(Item { id: 1, name: input.name }))
}

# use trillium_testing::TestServer;
# trillium_testing::block_on(async {
#     let app = TestServer::new((trillium::State::new(Db), api(create))).await;
#     app.post("/")
#         .with_request_header("content-type", "application/json")
#         .with_body(r#"{"name":"widget"}"#)
#         .await
#         .assert_status(Status::Created);
# });
```

A common pattern for complex handlers is to use a type alias:

```rust,ignore
type CreateArgs = (Db, CreateItem, AppConfig);

async fn create(_conn: &mut Conn, (db, input, config): CreateArgs) -> impl Handler {
    // ...
}
```

## Wrapper extractors for foreign types

Sometimes you can't add `#[derive(TryFromConn)]` to a type — typically
because it lives in another crate. The
[`Body<T>`](crate::Body), [`Json<T>`](crate::Json), and
[`State<T>`](crate::State) newtypes give you the same behavior as the
derive without modifying the wrapped type:

```rust
use trillium_api::{api, Body, Json, State};
use trillium::Conn;

# #[derive(Clone, Debug)]
# struct AppConfig { name: String }
async fn show_config(
    _conn: &mut Conn,
    State(config): State<AppConfig>,
) -> Json<String> {
    Json(config.name)
}
# use trillium_testing::TestServer;
# trillium_testing::block_on(async {
#     let app = TestServer::new((
#         trillium::State::new(AppConfig { name: "my app".into() }),
#         api(show_config),
#     )).await;
#     app.get("/").await.assert_ok().assert_body(r#""my app""#);
# });
```

`Body<T>` deserializes via content-type negotiation and serializes via
`Accept`. `Json<T>` is JSON-only on both sides. `State<T>` calls
[`take_state`](trillium::Conn::take_state), which removes the value from
the conn — if it's missing, the api handler is not called and the conn
passes through unmodified (default 404).

For your own types, prefer the derive — it produces the same code without
the newtype indirection at call sites.

## `Option` and `Result` as extractors

Normally, when extraction fails, your handler function is never called.
But sometimes you want to *handle* the missing or invalid data yourself
rather than letting the extractor's error response take over.

### `Option<T>` — maybe extract

`Option<T>` always succeeds as an extractor. If the inner `FromConn`
returns `None`, you get `None` instead of the handler being skipped:

```rust
use trillium_api::{api, Json, FromConn};
use trillium::Conn;

#[derive(Debug, Clone)]
struct User(String);

impl FromConn for User {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        conn.request_headers()
            .get_str("x-user")
            .map(|s| User(s.to_owned()))
    }
}

/// Greets the user by name if authenticated, or as "stranger" if not.
async fn greet(_conn: &mut Conn, user: Option<User>) -> String {
    match user {
        Some(User(name)) => format!("hello, {name}"),
        None => "hello, stranger".into(),
    }
}

# use trillium_testing::TestServer;
# trillium_testing::block_on(async {
#     let app = TestServer::new(api(greet)).await;
#     app.get("/").with_request_header("x-user", "alice").await.assert_ok().assert_body("hello, alice");
#     app.get("/").await.assert_ok().assert_body("hello, stranger");
# });
```

This is also the basis of the middleware pattern — see
[`recipes`](crate::recipes).

### `Result<T, E>` — catch extraction errors

`Result<T, E>` always succeeds when `T: TryFromConn<Error = E>`. Instead
of the error handler running automatically, you receive the `Err` and
can decide what to do:

```rust
use trillium_api::{api, Body, Json};
use trillium::Conn;
use serde::Deserialize;

#[derive(Deserialize)]
struct Input { name: String }

/// If the body fails to parse, returns a custom message instead of
/// trillium-api's default error response.
async fn lenient(
    _conn: &mut Conn,
    body: Result<Body<Input>, trillium_api::Error>,
) -> String {
    match body {
        Ok(Body(input)) => format!("got: {}", input.name),
        Err(e) => format!("bad request, but that's ok: {e}"),
    }
}

# use trillium_testing::TestServer;
# trillium_testing::block_on(async {
#     let app = TestServer::new(api(lenient)).await;
#     app.post("/")
#         .with_request_header("content-type", "application/json")
#         .with_body(r#"{"name":"alice"}"#)
#         .await
#         .assert_ok()
#         .assert_body("got: alice");
#
#     app.post("/")
#         .with_request_header("content-type", "application/json")
#         .with_body("not json")
#         .await
#         .assert_body_with(|body| {
#             assert!(body.starts_with("bad request, but that's ok:"), "{body}");
#         });
# });
```

## What happens when extraction fails

The behavior depends on which trait the extractor implements:

- **[`FromConn`](crate::FromConn)** — returns `Option<Self>`. If `None`,
  the api handler is not called and the conn passes through unmodified
  (no status, no body — the default 404).

- **[`TryFromConn`](crate::TryFromConn)** — returns
  `Result<Self, Self::Error>` where `Error: Handler`. On `Err`, the error
  value is *run as a handler* on the conn. For example,
  [`Body<T>`](crate::Body)'s error type is [`Error`](crate::Error), which
  responds with a JSON error body and an appropriate status code.

Wrapping an extractor in `Option` or `Result` (as shown above) lets you
intercept these failures and handle them in your own code instead.

See [`error_handling`](crate::error_handling) for more detail.
