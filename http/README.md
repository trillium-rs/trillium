# ⚙️ trillium-http — low-level HTTP/1.x implementation

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-http.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-http
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-http

Low-level HTTP/1.x and HTTP/3 implementation for Trillium. Provides `Conn` for managing a single request/response lifecycle, `ServerConfig` for shared server settings, header types (`Headers`, `KnownHeaderName`, `HeaderValue`), and body types. This crate is primarily intended for use by the higher-level [`trillium`](https://docs.rs/trillium) crate and runtime adapters — most users should depend on those instead.

## Example

```rust,no_run
use trillium_http::{ServerConfig, KnownHeaderName, Status};

let config = ServerConfig::default();
// Use `config.run(transport, handler_fn)` to process a connection
// over any AsyncRead + AsyncWrite transport.
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
