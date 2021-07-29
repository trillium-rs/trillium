# Handlers

The simplest form of a handler is any async function that takes a
`Conn` and returns that `Conn`. This example sets a `200 Ok` status
and sets a string body.

```rust,noplaypen
use trillium::Conn;
async fn hello_world(conn: Conn) -> Conn {
    conn.ok("hello world!")
}
```

With no further modification, we can drop this handler into a trillium
server and it will respond to http requests for us. We're using the
[`smol`](https://github.com/smol-rs/smol)-runtime based server adapter
here.

```rust,noplaypen
pub fn main() {
    trillium_smol::run(hello_world);
}
```

We can also define this as a closure:

```rust,noplaypen
pub fn main() {
    trillium_smol::run(|conn: trillium::Conn| async move {
        conn.ok("hello world")
    });
}
```

This handler will respond to any request regardless of path, and it
will always send a `200 Ok` http status with the specified body of
`"hello world"`.

## The State Handler

Trillium offers only one handler in the main `trillium` crate: The
State handler, which places a clone of any type you provide into the
state set of each conn that passes through it. See the
[rustdocs for State](https://docs.trillium.rs/trillium/struct.state)
for example usage.

## Tuple Handlers

Earlier, we discussed that we can use state to send data between
handlers and that handlers can always pass along the conn
unchanged. In order to use this, we need to introduce the notion of
tuple handlers.

Each handler in a tuple handler is called from left to right until the
conn is halted.

> 🔌 Readers familiar with elixir plug will recognize this notion as
> identical to pipelines, and that the term halt [is ~~stolen from~~
> inspired by plug](https://hexdocs.pm/plug/Plug.Conn.html#halt/1)

```rust,noplaypen
env_logger::init();
use trillium_logger::Logger;
run((
    Logger::new(),
    |conn: Conn| async move { conn.ok("tuple!") }
));
```

This snippet adds a http logger to our application, so that if we
execute our application with `RUST_LOG=info cargo run` and make a
request to http://localhost:8000, we'll see log output on stdout.

> 🧑‍🎓❓ Why not vectors or arrays? Rust vectors and arrays are type
> homogeneous, so in order to store the Logger and closure type in the
> above example in an array or vector, we'd need to allocate them to
> the heap and actually store a smart pointer in our homogeneous
> collection. Trillium initially was built around a notion of
> "sequences," which were a wrapper around `Vec<Box<dyn Handler +
> 'static>>`. Because tuples are generic over each of their elements,
> they can contain heterogeneous elements of different sizes, without
> heap allocation or smart pointers.

## Implementing Handler

The [rustdocs for
Handler](https://docs.trillium.rs/trillium/trait.handler) contains the
full details of the Handler interface for library authors. For many
applications, it will not be necessary to use anything other than an
async function or closure, but Handler can contain its own state and be
implemented for any type that you author.

## Assorted implementations provided by the trillium crate

You may see a few other types used in tests and examples.
* `()`: the noop handler, identical to `|conn: Conn| async move { conn }`
* `&'static str` and `String`: This simple handler responds to all
  conns by halting, setting a 200-ok status, and sending the string
  content as response body. `trillium_smol::run("hello")` is identical
  to `trillium_smol::run(|conn: Conn| async move { conn.ok("hello")
  })`
* `Option<impl Handler>`: This handler will noop if the option variant
  is none. This is useful for conditionally including handlers at
  runtime based on configuration or environment.
