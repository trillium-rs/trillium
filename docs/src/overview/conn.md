# Conn

Before we explore the concept of a handler further, let's take a look
at `Conn`. As mentioned above, `Conn` represents both the request and
response, as well as any data your application associates with that
request-response cycle.

> 🧑‍🎓 Advanced aside: Although the naming of Conn is directly
> borrowed from Elixir's [`plug`](https://github.com/elixir-plug/plug)
> and therefore also [`Phoenix`](https://www.phoenixframework.org/),
> it does in fact also own (in the rust sense) the singular
> `TcpStream` that represents the connection with the http client, and
> dropping a `Conn` will also disconnect the client as a result.

The [rustdocs for Conn](https://docs.trillium.rs/trillium/struct.conn)
contain the full details for all of the things you can do with a conn.

## Returning Conn
In general, because you'll be returning `Conn` from handlers, it
supports a chainable (fluent) interface for setting properties, like:

```rust,noplaypen
conn.with_status(202)
    .with_header("content-type", "application/something-custom")
    .with_body("this is my custom body")
```

## Accessing http request properties

Conn also contains read-only properties like request headers, request
path, and request method, each of which have getter associated
functions.

## Default Response

The default response for a Conn is a 404 with no response body, so it
is always valid to return the Conn from a handler unmodified (`|conn:
Conn| async move { conn }` is the simplest valid handler).

## State

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
> state is achieved using the
> [`trillium::State`](https://docs.trillium.rs/trillium/struct.state)
> handler, which holds any type that is Clone and puts a clone of it
> into the state of each Conn that passes through the handler.


## Extending Conn

It is a very common pattern in trillium for libraries to extend Conn
in order to provide additional functionality[^1]. The Conn interface
does not provide support for sessions, cookies, route params, or many
other building blocks that other frameworks build into the core
types. Instead, to use sessions as an example, `trillium_sessions`
provides a `SessionConnExt` trait which provides associated functions
for Conn that offer session support. In general, handlers that put
data into conn state also will provide convenience functions for
accessing that state, and will export a `[Something]ConnExt` trait.

[^1] 🧑‍🎓 see [library_patterns](../library_patterns.md) for an example
of authoring one of these
