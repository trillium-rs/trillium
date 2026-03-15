# 📡 trillium-client — async HTTP client

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-client.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-client
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-client

Async HTTP/1.x and HTTP/3 client for Trillium. Uses the same `Conn`-based design as the Trillium server side and shares connection pools across requests. Pair it with a runtime adapter's `ClientConfig` (from [`trillium-smol`](https://docs.rs/trillium-smol), [`trillium-tokio`](https://docs.rs/trillium-tokio), or [`trillium-async-std`](https://docs.rs/trillium-async-std)) and optionally a TLS adapter.

## Example

```rust,no_run
use trillium_client::Client;
use trillium_smol::ClientConfig;

async fn fetch() -> trillium_client::Result<()> {
    let client = Client::new(ClientConfig::default());
    let body = client
        .get("http://example.com/")
        .await?
        .response_body()
        .await?;
    println!("{body}");
    Ok(())
}
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
