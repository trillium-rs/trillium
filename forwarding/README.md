# 📡 trillium-forwarding — trusted reverse proxy header support

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-forwarding.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-forwarding
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-forwarding

Rewrites the connection's host, protocol, and peer IP from `Forwarded` or `X-Forwarded-*` headers
added by a trusted reverse proxy. Headers are only applied if the connecting peer IP is trusted —
use the narrowest possible trust rule for your deployment to prevent header spoofing.

## Example

```rust,no_run
use trillium_forwarding::Forwarding;

// Trust specific IPs or CIDR ranges
let app = (
    Forwarding::trust_ips(["10.0.0.0/8", "172.16.0.0/12"]),
    |conn: trillium::Conn| async move { conn.ok("ok") },
);

// Or trust based on a custom predicate
let app = (
    Forwarding::trust_fn(|ip| ip.is_loopback()),
    |conn: trillium::Conn| async move { conn.ok("ok") },
);

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
