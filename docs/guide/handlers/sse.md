# Server-Sent Events

[rustdocs](https://docs.trillium.rs/trillium_sse)

Server-Sent Events (SSE) let a server push a stream of events to a browser over a persistent HTTP connection. The browser keeps the connection open and fires JavaScript events as data arrives, and reconnects automatically if the connection drops.

SSE is unidirectional: the server sends, the client receives. Use SSE when you need to push updates to the browser and don't need to receive messages back over the same connection. For bidirectional communication, see [WebSockets](./websockets.md) or [Channels](./channels.md).

## The Sse handler

`sse()` builds a handler from any `SseHandler` — most simply, a closure that receives the conn and returns a `Stream` of `Eventable` items:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-sse = { path = "../sse" }
# futures-lite = "*"
#
# fn main() {
use futures_lite::stream;
use trillium::Conn;
use trillium_sse::sse;

trillium_smol::run(sse(|_: &mut Conn| {
    stream::iter(["one", "two", "three"])
}));
# }
```

The handler negotiates on the request's `Accept` header, passing the conn through to subsequent handlers if the client doesn't accept `text/event-stream` — so it composes in a tuple without needing a route of its own. A request with no `Accept` header accepts anything.

When a client goes away, the event stream is dropped promptly on every protocol, so a stream backed by a subscription can use `Drop` to unsubscribe. A client that vanishes without signalling — a killed process, a severed network — is noticed once the transport notices, which on HTTP/3 is QUIC's negotiated idle timeout.

## The Event type

For finer-grained control, use the `Event` type which supports typed event names:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-sse = { path = "../sse" }
# futures-lite = "*"
#
# fn main() {
use futures_lite::stream;
use trillium::Conn;
use trillium_sse::{Event, sse};

trillium_smol::run(sse(|_: &mut Conn| {
    stream::iter([
        Event::new("connection established").with_type("status"),
        Event::new(r#"{"user":"alice","action":"joined"}"#).with_type("message"),
    ])
}));
# }
```

The `Eventable` trait is also implemented for `String` and `&'static str`, so simple text streams work without wrapping.

## Comments

An event can also carry a comment, which clients ignore. A comment-only message — no `data:`
field, so nothing is dispatched to the page — is the conventional SSE heartbeat: proxies and
load balancers will often close a connection that has been idle for some time, and a periodic
comment keeps traffic flowing without the client seeing anything.

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-sse = { path = "../sse" }
# futures-lite = "*"
#
# fn main() {
use futures_lite::stream;
use trillium::Conn;
use trillium_sse::{Event, sse};

trillium_smol::run(sse(|_: &mut Conn| {
    stream::iter([
        Event::new_comment("heartbeat"),
        Event::new("hello").with_id("1"),
    ])
}));
# }
```

## Heartbeats

Long-lived event streams are often idle for minutes at a time, and an idle connection is
vulnerable — proxies and load balancers tend to close it. `Sse::with_heartbeat` sends a comment
whenever the given interval passes without an event, so the stream is never entirely silent:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-sse = { path = "../sse" }
# futures-lite = "*"
#
# fn main() {
use std::time::Duration;
use futures_lite::stream;
use trillium::Conn;
use trillium_sse::sse;

trillium_smol::run(
    sse(|_: &mut Conn| stream::pending::<String>())
        .with_heartbeat(Duration::from_secs(15)),
);
# }
```

The interval is measured from the most recent event rather than on a fixed schedule, so a stream
that is already sending regularly produces no heartbeats at all.

## Real-time fan-out

In practice, SSE is most useful paired with a broadcast channel so that server-side events reach all connected clients. The exact channel type is up to you — any `Stream` works:

```rust
# [dependencies]
# trillium = { path = "../trillium" }
# trillium-smol = { path = "../smol" }
# trillium-sse = { path = "../sse" }
# futures-lite = "*"
#
use trillium::Conn;
use trillium_sse::sse;

# #[derive(Clone)]
# struct Broadcaster;
# impl futures_lite::stream::Stream for Broadcaster {
#     type Item = String;
#     fn poll_next(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<Self::Item>> {
#         std::task::Poll::Pending
#     }
# }
#
// Assume `Broadcaster` is a channel type from your preferred library
// that implements Clone and Stream<Item = String>.
fn app(broadcaster: Broadcaster) -> impl trillium::Handler {
    sse(move |_: &mut Conn| broadcaster.clone())
}

# fn main() {
#     trillium_smol::run(app(Broadcaster));
# }
```

The `swansong()` mechanism ensures the SSE stream is terminated when the server shuts down — the `Sse` handler wraps the stream in a shutdown interrupt automatically.
