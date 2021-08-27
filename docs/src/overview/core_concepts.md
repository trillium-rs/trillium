# Core concepts: Handlers, Conn, and Adapters

The most important concepts when building a trillium application are
the `Conn` type and the `Handler` trait. Each `Conn` represents a
single http request/response pair, and a `Handler` is the trait that
all applications, middleware, and endpoints implement.

Let's start with an overview of a simple trillium application and then
dig into each of those concepts a little more.

```rust,noplaypen
fn main() {
    trillium_smol::run(|conn: trillium::Conn| async move {
        conn.ok("hello from trillium!")
    });
}
```

In this example, `trillium_smol::run` is the runtime adapter and the
closure is a Handler that responds "hello from trillium!" to any web
request it receives. This is a fully functional example that you can
run with only the following dependencies:

```toml
[dependencies]
trillium = "0.2"
trillium-smol = "0.2"
```

If we `cargo run` this example, we can then visit
http://localhost:8080 in a browser or make a curl request against that
url and see "hello from trillium!" as the response body. Note that we
won't see any output in the terminal because trillium is silent by
default.
