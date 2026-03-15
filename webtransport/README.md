# 🚀 trillium-webtransport — WebTransport over HTTP/3

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-webtransport.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-webtransport
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-webtransport

WebTransport session handler for Trillium. Accepts WebTransport sessions over HTTP/3 (QUIC) and exposes bidirectional streams, unidirectional streams, and datagrams per session. Requires an HTTP/3-capable adapter such as [`trillium-quinn`](https://docs.rs/trillium-quinn).

## Example

```rust,no_run
use trillium_webtransport::{WebTransport, WebTransportConnection};

let app = WebTransport::new(|conn: WebTransportConnection| async move {
    while let Some(stream) = conn.accept_next_stream().await {
        // handle bidirectional or unidirectional stream
        drop(stream);
    }
});
// run with an HTTP/3-capable adapter, e.g.:
// trillium_smol::config().with_acceptor(quinn_config).run(app);
```

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
