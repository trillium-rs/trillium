# Channels

[rustdocs](https://docs.trillium.rs/trillium_channels)

The `trillium-channels` crate implements Phoenix-style channels: topic-based pub/sub over WebSocket connections.

## How it works

Clients connect via WebSocket and then join one or more topics — strings like `"rooms:lobby"` or `"notifications:42"`. Once joined, they send and receive messages scoped to that topic. The server implements a `ChannelHandler` that controls which joins are allowed and how incoming messages are processed.

The wire protocol is compatible with the [Phoenix JavaScript client](https://hexdocs.pm/phoenix/js/), so existing frontend code targeting Phoenix can connect to a Trillium channels server without modification.

## Example: chat room

```rust
# [dependencies]
# trillium-smol = { path = "../smol" }
# trillium-channels = { path = "../channels" }
#
use trillium_channels::{ChannelConn, ChannelEvent, ChannelHandler, channel};

struct ChatChannel;

impl ChannelHandler for ChatChannel {
    async fn join_channel(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
        match event.topic() {
            "rooms:lobby" => {
                conn.allow_join(&event, &()).await;
                conn.broadcast(("rooms:lobby", "user:entered"));
            }
            _ => {
                conn.reply_error(&event, &"unexpected topic join");
            }
        }
    }

    async fn incoming_message(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
        match (event.topic(), event.event()) {
            ("rooms:lobby", "new:msg") => conn.broadcast(event),
            _ => {}
        }
    }
}

fn main() {
    trillium_smol::run(channel(ChatChannel));
}
```

`conn.allow_join` registers the client on the topic. `conn.broadcast` sends an event to all clients subscribed to that topic, including the sender.

## Constructing events

The `event!` macro builds `ChannelEvent` values with an optional JSON payload:

```rust
# [dependencies]
# trillium-channels = { path = "../channels" }
#
# fn main() {
use trillium_channels::event;

let ping = event!("heartbeat", "ping");
let msg  = event!("rooms:lobby", "new:msg", { "body": "hello everyone" });
# }
```

## Broadcasting outside handlers

`Channel::broadcaster()` returns a `ChannelBroadcaster` that can be cloned and shared across tasks. Use it to push events from background jobs, timers, or any other code that doesn't have access to a `ChannelConn`:

```rust
# [dependencies]
# trillium-channels = { path = "../channels" }
#
# fn main() {
#     use trillium_channels::event;
#     let channel_handler = trillium_channels::channel(());
let broadcaster = channel_handler.broadcaster();

// In a background task:
broadcaster.broadcast(event!("system", "announcement", { "text": "Server restarting soon" }));
# }
```

## Current limitations

- WebSocket transport only (no long-polling)
- No built-in multi-node message distribution — use `Channel::broadcaster()` with an external pub/sub system (Redis, NATS, etc.) to synchronize across server instances
