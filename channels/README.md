# 📢 trillium-channels — Phoenix-style WebSocket channels

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-channels.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-channels
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-channels

A [Phoenix Channels](https://hexdocs.pm/phoenix/channels.html) implementation for trillium.
Clients connect via WebSocket and subscribe to topics; the server can broadcast events to all
subscribers of a topic. Implement the [`ChannelHandler`][docs] trait to define your application's
join and message logic.

## Example

```rust,no_run
use trillium_channels::{ChannelConn, ChannelEvent, ChannelHandler, channel};

struct ChatChannel;

impl ChannelHandler for ChatChannel {
    async fn join_channel(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
        match event.topic() {
            "rooms:lobby" => {
                conn.allow_join(&event, &()).await;
                conn.broadcast(("rooms:lobby", "user:entered"));
            }
            _ => {}
        }
    }

    async fn incoming_message(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
        if let ("rooms:lobby", "new:msg") = (event.topic(), event.event()) {
            conn.broadcast(event);
        }
    }
}

// run with your chosen runtime adapter, e.g.:
// trillium_tokio::run(channel(ChatChannel));
```

## Safety

This crate uses `#![forbid(unsafe_code)]`.

## License

<sup>
Licensed under either of <a href="LICENSE-APACHE">Apache License, Version
2.0</a> or <a href="LICENSE-MIT">MIT license</a> at your option.
</sup>

---

<sub>
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this crate by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
</sub>
