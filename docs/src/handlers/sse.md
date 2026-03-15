# Server-Sent Events

[rustdocs](https://docs.trillium.rs/trillium_sse)

Server-Sent Events (SSE) let a server push a stream of events to a browser over a persistent HTTP connection. The browser keeps the connection open and fires JavaScript events as data arrives, and reconnects automatically if the connection drops.

SSE is unidirectional: the server sends, the client receives. Use SSE when you need to push updates to the browser and don't need to receive messages back over the same connection. For bidirectional communication, see [WebSockets](./websockets.md) or [Channels](./channels.md).

## Basic usage

The `SseConnExt` trait adds a `with_sse_stream` method to `Conn`. Pass it any `Stream` of items that implement `Eventable`:

```rust,noplaypen
use futures_lite::stream;
use trillium_sse::SseConnExt;

trillium_smol::run(|conn: trillium::Conn| async move {
    let events = stream::iter(["one", "two", "three"]);
    conn.with_sse_stream(events)
});
```

`with_sse_stream` sets the `Content-Type: text/event-stream` and `Cache-Control: no-cache` headers, sets the status to 200, and halts the conn.

## The Event type

For finer-grained control, use the `Event` type which supports typed event names:

```rust,noplaypen
use trillium_sse::{Event, SseConnExt};

let events = stream::iter([
    Event::new("connection established").with_type("status"),
    Event::new(r#"{"user":"alice","action":"joined"}"#).with_type("message"),
]);

conn.with_sse_stream(events)
```

The `Eventable` trait is also implemented for `String` and `&'static str`, so simple text streams work without wrapping.

## Real-time fan-out

In practice, SSE is most useful paired with a broadcast channel so that server-side events reach all connected clients. The exact channel type is up to you — any `Stream` works:

```rust,noplaypen
use trillium::State;
use trillium_sse::SseConnExt;

// Assume `Broadcaster` is a channel type from your preferred library
// that implements Clone (for State) and Stream<Item = String> (for SSE).
fn app(broadcaster: Broadcaster) -> impl trillium::Handler {
    (
        State::new(broadcaster),
        |mut conn: trillium::Conn| async move {
            let rx = conn.take_state::<Broadcaster>().unwrap();
            conn.with_sse_stream(rx)
        },
    )
}
```

The `conn.swansong()` mechanism ensures the SSE stream is terminated when the server shuts down — `with_sse_stream` wraps the stream in a shutdown interrupt automatically.
