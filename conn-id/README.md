# 🏷️ trillium-conn-id — per-connection request IDs

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-conn-id.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-conn-id
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-conn-id

Assigns a unique identifier to every connection. By default, the ID is taken from the inbound
`x-request-id` header if present, or generated as a 10-character random alphanumeric string. The
ID is sent back as an `x-request-id` response header and is accessible on the `Conn` via
[`ConnIdExt`][docs].

## Example

```rust,no_run
use trillium::Conn;
use trillium_conn_id::{ConnId, ConnIdExt};

let app = (
    ConnId::new(),
    |conn: Conn| async move {
        let id = conn.id().to_string();
        conn.ok(format!("your request id is {id}"))
    },
);
// run with your chosen runtime adapter, e.g.:
// trillium_tokio::run(app);
```

The ID generator is customizable — for example, to use UUIDs:

```rust,no_run
use trillium_conn_id::ConnId;

let handler = ConnId::new()
    .with_id_generator(|| uuid::Uuid::new_v4().to_string());
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
