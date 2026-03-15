# 🌊 trillium-sse — Server-Sent Events

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-sse.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-sse
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-sse

[Server-Sent Events](https://html.spec.whatwg.org/multipage/server-sent-events.html) for trillium.
Takes any `Stream` whose items implement [`Eventable`][docs] and streams them to the client as an
SSE response. `String`, `&'static str`, and the included `Event` type all implement `Eventable`;
implement it on your own types for custom event/id fields.

## Example

```rust,no_run
use broadcaster::BroadcastChannel;
use trillium::{Conn, State};
use trillium_sse::SseConnExt;

type Channel = BroadcastChannel<String>;

let app = (
    State::new(Channel::new()),
    |mut conn: Conn| async move {
        let channel = conn.take_state::<Channel>().unwrap();
        conn.with_sse_stream(channel)
    },
);
// run with your chosen runtime adapter, e.g.:
// trillium_tokio::run(app);
```

The connection stays open and events are pushed as items arrive on the stream. The stream is
automatically interrupted when the client disconnects or the server shuts down.

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
