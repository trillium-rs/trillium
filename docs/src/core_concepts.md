# Two core concepts: Conn and Handlers

The two most important concepts in trillium are the `Conn` type and the
`Handler` trait. Each `Conn` represents a single http request/response
pair.

## Handlers

The simplest form of a handler is any async function that takes a
`Conn` and returns that `Conn`. This example sets a `200 Ok` status
and sets a string body.

```rust
use trillium::Conn;
async fn hello_world(conn: Conn) -> Conn {
    conn.ok("hello world!")
}
```

With no further modification, we can drop this handler into a trillium
server and it will respond to http requests for us. We're using the
[`smol`](https://github.com/smol-rs/smol)-runtime based server adapter
here.

```rust
pub fn main() {
    trillium_smol_server::run(hello_world);
}
```

We can also define this as a closure:

```rust
pub fn main() {
    trillium_smol_server::run(|conn: trillium::Conn| async move {
        conn.ok("hello world")
    });
}
```

This handler will respond to any request at localhost:8080, regardless of
path, and it will always send a `200 Ok` http status with the
specified body of `"hello world"`.

## Conn

Before we explore the concept of a handler further, let's take a quick
aside for `Conn`. As mentioned above, `Conn` represents both the
request and response, as well as any data your application associates
with that request-response cycle.

> 🧑‍🎓 Advanced aside: Although the naming of Conn is directly
> borrowed from Elixir's [`plug`](https://github.com/elixir-plug/plug)
> and therefore also [`Phoenix`](https://www.phoenixframework.org/),
> it does in fact also own (in the rust sense) the singular
> `TcpStream` that represents the connection with the http client, and
> dropping a `Conn` will also disconnect the client as a result.

The [rustdocs for Conn](https://docs.trillium.rs/trillium/struct.conn)
contain the full details for all of the things you can do with a conn.

### Returning Conn
In general, because you'll be returning `Conn` from handlers, it
supports a chainable (fluent) interface for setting properties, like:

```rust
conn.with_status(202)
    .with_header(("content-type", "application/something-custom"))
    .with_body("this is my custom body")
```

### Accessing http request properties

Conn also contains read-only properties like request headers, request
path, and request method, each of which have getter associated
functions.

### Default Response

The default response for a Conn is a 404 with no response body, so it
is always valid to return the Conn from a handler unmodified (`|conn:
Conn| async move { conn }` is the simplest valid handler).

### State

In addition to holding the request properties and accumulating the
response your application is going to send, a Conn also serves as a
data structure for any information your application needs to associate
with that request. This is especially valuable for communicating
between handlers, and most core handlers are implemented using conn
state. One important caveat to is that each Conn can only contain
exactly one of each type, so it is highly recommended that you only
store types that you define in state.

> 🌊 Comparison with Tide: Tide has three different types of state:
> Server state, request state, and response state. In Trillium, server
> state is achieved using the [`trillium::State`](https://docs.trillium.rs/trillium/struct.state) handler, which holds any
> type that is Clone and puts a clone of it into the state of each
> Conn that passes through the handler.

### Extending Conn

It is a very common pattern in trillium for libraries to extend Conn in
order to provide additional functionality.  The Conn interface does
not provide support for sessions, cookies, route params, or many other
building blocks that other frameworks build into the core
types. Instead, to use sessions as an example, `trillium_sessions`
provides a `SessionConnExt` trait which provides associated functions
for Conn that offer session support. In general, handlers that put
data into conn state also will provide convenience functions for
accessing that state, and will export a `[Something]ConnExt` trait.

## Servers

Let's talk a little more about that `trillium_smol_server::run` line we've
been writing. Trillium itself is built on `futures` (`futures-lite`,
specifically). In order to run it, it needs an adapter to an async
runtime. These adapters are called servers, and there are four of them
currently:

* `trillium_async_std_server`
* `trillium_smol_server`
* `trillium_tokio_server`
* `trillium_aws_lambda_server`

Although we've been using the smol server in these docs thus far, you
should use whichever runtime you prefer. If you expect to have a
dependency on async-std or tokio anyway, you might as well use the
server for that runtime.

To run trillium on a different host or port, either provide a `HOST` and/or `PORT` environment variables, or compile the specific values into the server as follows:

```rust
{{#include ../../smol-server/examples/smol-server-with-config.rs}}
```

### TLS / HTTPS

With the exception of aws lambda, which provides its own tls
termination at the load balancer, each of the above servers can be
combined with either rustls or native-tls.

Rustls:
```rust
{{#include ../../rustls/examples/rustls.rs}}
```

Native tls:
```rust
{{#include ../../native-tls/examples/native-tls.rs}}
```

## Tuples and Sequences

Earlier, we discussed that we can use state to send data between
handlers and that handlers can always pass along the conn
unchanged. In order to use this, we need to introduce the notion of
Sequences. A Sequence is a `Vec` of boxed dyn handlers, each of which
is run on the connection until one of the handlers halts the conn. A
tuple handler is the generic stack-allocated fixed-length
equivalent. If you're unsure which to use, start with a tuple.

> 🔌 Readers familiar with elixir plug will recognize this notion as
> identical to pipelines, and that the term halt [is ~~stolen from~~
> inspired by plug](https://hexdocs.pm/plug/Plug.Conn.html#halt/1)

### Tuple handlers

For sequences of up to 15 handlers with types that are known at
compile-time, tuples can and should be used. This avoids
heap-allocating handlers.

```rust
env_logger::init();
run((
    trillium_logger::DevLogger,
    |conn: Conn| async move { conn.ok("sequence!") }
));
```

This snippet adds a http logger to our application, so that if we
execute our application with `RUST_LOG=info cargo run` and make a
request to localhost:8000, we'll see something like `GET / 200
390.706µs 9bytes` on stdout.


### Building a Sequence

For more information on sequences, see
[trillium::Sequence](https://docs.trillium.rs/trillium/struct.sequence)

```rust
env_logger::init();
run(trillium::Sequence::new()
    .and(trillium_logger::DevLogger)
    .and(|conn: Conn| async move { conn.ok("sequence!") }));
```

### Macros, if you want them

Because sequences are quite common in trillium, we also have a macro that
makes them even easier to build. Instead of writing
`Sequence::new().and(a).and(b).and(c)` we can write
`trillium::sequence![a, b, c]`, which makes our above example:

```rust
env_logger::init();
run(trillium::sequence![
    trillium_logger::DevLogger,
    |conn: Conn| async move { conn.ok("sequence!") }
]);
```

Sequence is itself a handler, and as a result can be used in any place
another handler could be used.  If you need to, you can nest sequences
inside of each other.

