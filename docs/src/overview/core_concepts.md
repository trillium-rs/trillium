# Core concepts: Handlers, Conn, and Adapters

The most important concepts in Trillium are the `Handler` trait and the `Conn` type. Every Trillium application — from a one-liner to a full middleware stack — is a `Handler` that receives a `Conn` and returns a `Conn`.

Here's a minimal application:

```rust,noplaypen
fn main() {
    trillium_smol::run(|conn: trillium::Conn| async move {
        conn.ok("hello from trillium!")
    });
}
```

In this example:
- `trillium_smol::run` is the **runtime adapter** — it listens for TCP connections and drives the async executor.
- The closure is a **Handler** — it receives each incoming `Conn` and returns it with a 200 response and a body.

Add this to your `Cargo.toml` with:

```bash
cargo add trillium trillium-smol
```

Run it with `cargo run`, then visit `http://localhost:8080`. You won't see any output in the terminal because Trillium is silent by default — add a logger if you want request output.

The pages that follow go deeper into each of these concepts:

- [Handlers](./handlers.md) — the trait, tuple composition, and built-in implementations
- [Conn](./conn.md) — request/response data, state, and the conn extension pattern
- [Runtime Adapters, TLS, and HTTP/3](./runtimes.md) — choosing a runtime, configuration, TLS, and QUIC
