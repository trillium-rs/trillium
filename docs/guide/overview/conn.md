# Conn

`Conn` represents both the HTTP request and response for a single request-response cycle. It also owns the underlying connection — dropping a `Conn` disconnects the client.

> 🧑‍🎓 The name "Conn" is borrowed from Elixir's [Plug](https://github.com/elixir-plug/plug) and [Phoenix](https://www.phoenixframework.org/). Like those, it carries the full lifecycle of one request through the handler chain. Unlike them, a Trillium `Conn` owns the transport (TCP socket, TLS stream, or QUIC stream) directly.

The [rustdocs for Conn](https://docs.trillium.rs/trillium/struct.conn) cover every method. Here are the concepts you'll use most.

## Building a response

`Conn` supports a chainable interface for setting response properties:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
#
# fn main() {
#     trillium_smol::run(|conn: trillium::Conn| async move {
conn.with_status(202)
    .with_response_header("content-type", "application/something-custom")
    .with_body("this is my custom body")
#     });
# }
```

Convenience methods like `conn.ok("body")` combine common operations. `ok` sets status 200, sets the body, and halts the conn.

## Default response

If a handler returns `Conn` without setting anything, the response is `404 Not Found` with no body. This is always a valid thing to return — it's how handlers signal "I didn't handle this; try the next one."

## Reading the request

`Conn` provides read access to request properties:

- `conn.method()` — the HTTP method
- `conn.path()` — the request path
- `conn.headers()` — request headers
- `conn.request_body()` — the request body as an async reader
- `conn.peer_ip()` — the remote address

## State

In addition to request/response data, `Conn` carries an arbitrary state set — a [type-indexed
map](https://docs.rs/type-set) that handlers can use to store state on the conn. Each type can
appear at most once:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
#
# struct MyData {
#     user_id: usize,
# }
#
# use trillium::{Status, Conn};
# fn main() {
trillium_smol::run((
    |conn: Conn| async move { conn.with_state(MyData { user_id: 42 }) },
    |conn: Conn| async move {     // read it back in another handler
        if let Some(MyData { user_id }) = conn.state() {
            let response_body = format!("user id: {user_id}");
            conn.ok(response_body)
        } else {
            conn.with_status(Status::NotFound).halt()
        }
     }
));
# }
```

This is how most Trillium libraries work internally: a handler earlier in the chain stores data in the state set, and later handlers retrieve it.

## Extending Conn

Library crates typically expose their functionality through a `[Something]ConnExt` trait rather than adding methods directly to `Conn`. For example, `trillium-sessions` provides `SessionConnExt` with methods like `conn.session()`. You get these methods by importing the trait.

This pattern avoids conflicts between crates — since state is keyed by type, each library uses its own private newtype.

> 🧑‍🎓 See [Patterns for library authors](../library_patterns.md) for a worked example.
