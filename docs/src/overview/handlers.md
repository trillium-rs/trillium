# Handlers

The simplest handler is any async function that takes a `Conn` and returns it:

```rust,noplaypen
use trillium::Conn;

async fn hello_world(conn: Conn) -> Conn {
    conn.ok("hello world!")
}
```

Drop it into a server and it responds to every request:

```rust,noplaypen
pub fn main() {
    trillium_smol::run(hello_world);
}
```

Or write it as a closure:

```rust,noplaypen
pub fn main() {
    trillium_smol::run(|conn: trillium::Conn| async move {
        conn.ok("hello world")
    });
}
```

This handler responds to any request regardless of path, always with status 200.

## The State handler

The `trillium` crate exports one handler: `State<T>`, which clones a value into the state set of each `Conn` that passes through it. This is how shared resources (database pools, configuration, broadcast senders) are made available to downstream handlers.

See the [rustdocs for State](https://docs.trillium.rs/trillium/struct.state) for usage.

## Tuple handlers

Multiple handlers compose via tuples, which run left to right:

```rust,noplaypen
use trillium_logger::Logger;

trillium_smol::run((
    Logger::new(),
    |conn: Conn| async move { conn.ok("tuple!") },
));
```

Each handler in the tuple runs in order until one halts the `Conn`. Halting stops the chain — subsequent handlers are skipped. This is how handlers signal "I've handled this request" or "this request is not authorized."

> 🔌 Readers familiar with Elixir's Plug will recognize this as pipelines, and the term "halt" as [borrowed from Plug](https://hexdocs.pm/plug/Plug.Conn.html#halt/1).

Tuples are used here (rather than `Vec`) because Rust vectors are type-homogeneous — storing different handler types in a vector requires heap allocation and boxing. Tuples are generic over each element, so they can hold heterogeneous types without allocation.

## Implementing Handler

The `Handler` trait provides several lifecycle hooks beyond `run` — notably `init` (called once at startup) and `upgrade` (for WebSocket/WebTransport upgrades). For most applications, async functions and closures are sufficient. The [rustdocs for Handler](https://docs.trillium.rs/trillium/trait.handler) cover the full interface for library authors.

## Built-in implementations

A few types in the `trillium` crate implement `Handler` for convenience:

- `()` — the no-op handler; passes the conn through unchanged
- `&'static str` and `String` — halts with status 200 and the string as the body
- `Option<impl Handler>` — no-ops if `None`; useful for conditionally enabling a handler at startup based on configuration
