# 🔑 trillium-sessions — cookie-backed session management

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-sessions.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-sessions
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-sessions

Secure cookie-backed sessions for trillium. Session data is stored in a configurable
[`SessionStore`][docs] and the session cookie is signed with an HKDF-derived key. All requests
receive a session cookie by default (guest sessions), whether or not any data has been stored.

Two stores are bundled: `MemoryStore` (for development/testing) and `CookieStore` (self-contained,
no server state). For production, use an external datastore-backed store from
[async-session](https://github.com/http-rs/async-session).

## Example

```rust,no_run
use trillium::Conn;
use trillium_sessions::{MemoryStore, SessionConnExt, SessionHandler};

let app = (
    SessionHandler::new(MemoryStore::new(), b"supersecretkeyyoushouldnotcommit"),
    |conn: Conn| async move {
        let count: usize = conn.session().get("visits").unwrap_or(0) + 1;
        conn.with_session("visits", &count)
            .ok(format!("you have visited {count} times"))
    },
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
