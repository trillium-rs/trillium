# 🔀 trillium-proxy — HTTP reverse and forward proxy

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-proxy.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-proxy
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-proxy

Reverse-proxy handler for Trillium. Forwards incoming requests to one or more upstream servers, streams request and response bodies, rewrites `Forwarded` headers, and optionally proxies WebSocket upgrades. Supports random and connection-count–based load balancing across multiple upstreams.

## Example

```rust,no_run
use trillium_proxy::proxy;
use trillium_smol::ClientConfig;

let app = proxy(ClientConfig::default(), "http://localhost:8080");
// run with your chosen runtime adapter, e.g.:
// trillium_tokio::run(app);
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
