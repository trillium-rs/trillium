# Deriving `TryFromConn` and `Handler`

For your own types, the [`#[derive(TryFromConn)]`](crate::TryFromConn) and
[`#[derive(Handler)]`](trillium::Handler) macros generate the same impls
you'd write by hand, configured by an `#[api(...)]` attribute on the struct.

The two derives can be applied independently or together, and they share
the same attribute — useful when a type plays both roles (e.g. a clonable
state type that also writes itself back into state).

## `#[api(state)]` — extract from / write to conn state

By default, `state` *takes* the value out of the conn (matching
[`State<T>`](crate::State)'s behavior):

```rust
use trillium::Conn;
use trillium_api::{api, TryFromConn};

#[derive(Clone, Debug, TryFromConn)]
#[api(state)]
struct Db;

async fn show(_conn: &mut Conn, db: Db) -> String {
    let _ = db;
    "ok".into()
}
# use trillium_testing::TestServer;
# trillium_testing::block_on(async {
#     let app = TestServer::new((trillium::State::new(Db), api(show))).await;
#     app.get("/").await.assert_ok();
# });
```

Add `clone` to leave the value in conn state for downstream extractors:

```rust
use trillium::Conn;
use trillium_api::{api, TryFromConn};

#[derive(Clone, Debug, TryFromConn)]
#[api(state, clone)]
struct Db(/* ... */);
```

The associated `Error` type is `()` — when state is missing, the api
handler is skipped (default 404). To run a custom handler in that case,
use `err = SomeHandler` where `SomeHandler` implements both `Default` and
`Handler`:

```rust
use trillium::{Conn, Handler, Status};
use trillium_api::{api, TryFromConn};

#[derive(Default)]
struct ServerError;
impl Handler for ServerError {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_status(Status::InternalServerError).halt()
    }
}

#[derive(TryFromConn)]
#[api(state, err = ServerError)]
struct RequiredState(u32);
```

## `#[api(json)]` — JSON request body

Deserializes from JSON only (rejects other content types). Requires
`T: serde::de::DeserializeOwned`:

```rust
use trillium::Conn;
use trillium_api::{api, TryFromConn};
use serde::Deserialize;

#[derive(Deserialize, TryFromConn)]
#[api(json)]
struct NewPost {
    title: String,
}

async fn create(_conn: &mut Conn, post: NewPost) -> String {
    format!("created: {}", post.title)
}
```

The associated `Error` is [`Error`](crate::Error). To swap in your own
error handler, use `err = MyError`:

```rust
# use trillium::{Conn, Handler};
# use trillium_api::TryFromConn;
# use serde::Deserialize;
#[derive(Default)]
struct BadRequest;
impl Handler for BadRequest {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_status(400).halt()
    }
}

#[derive(Deserialize, TryFromConn)]
#[api(json, err = BadRequest)]
struct StrictInput {
    value: u32,
}
```

The original error is discarded; if you need its detail, use `err`-less
extraction and let the default `Error` handler render it.

## `#[api(body)]` — content-negotiated body

Like `json`, but uses the `Content-Type` header to choose the deserializer
(JSON or form-urlencoded with the `forms` feature). Otherwise identical to
`json` — same `Error` type, same `err = ...` override.

## Pairing with `#[derive(Handler)]`

The same `#[api(...)]` attribute drives both derives. Each does the
symmetric thing:

| `#[api(...)]` | `TryFromConn` does | `Handler` does |
|---|---|---|
| `state` | `take_state::<Self>()` | `with_state(self.clone())` (requires `Self: Clone`) |
| `state, clone` | `state::<Self>().cloned()` | same as above |
| `state, err = E` | `take_state::<Self>().ok_or_else(E::default)` | same as state |
| `json` | `deserialize_json::<Self>(conn)` | `with_json(self)` (requires `Serialize`) |
| `body` | `deserialize::<Self>(conn)` | content-negotiated `serialize(self)` (requires `Serialize`) |

`clone` and `err` are silently ignored by the `Handler` derive — they're
specifically for extraction.

A complete round-tripping type:

```rust
use trillium::Conn;
use trillium_api::{api, Handler, TryFromConn};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, TryFromConn, Handler)]
#[api(body)]
struct Echo {
    payload: String,
}

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

## Note on `trillium_api::Handler`

`trillium_api::Handler` is the derive macro from this crate; it shadows
the `trillium::Handler` *trait* of the same name when both are imported.
This mirrors how `serde::Serialize` works. If you need the trait in scope
alongside the derive, import them separately:

```rust,ignore
use trillium::Handler;            // the trait
use trillium_api::Handler as _;   // the derive macro is still callable
```

In practice you'll usually only need one of them in any given file, so
the shadowing is rarely visible.

## When the derive isn't enough

The derive covers extraction from state, JSON, and content-negotiated
bodies. For anything else — header sniffing, route parameter parsing,
multi-step extraction with database lookups, conditional logic — write
the impl by hand. See [`extractors::custom`](crate::extractors::custom).
