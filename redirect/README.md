# ↪️ trillium-redirect — HTTP redirection handler

[![ci][ci-badge]][ci]
[![crates.io version][version-badge]][crate]
[![docs.rs][docs-badge]][docs]

[ci]: https://github.com/trillium-rs/trillium/actions?query=workflow%3ACI
[ci-badge]: https://github.com/trillium-rs/trillium/workflows/CI/badge.svg
[version-badge]: https://img.shields.io/crates/v/trillium-redirect.svg?style=flat-square
[crate]: https://crates.io/crates/trillium-redirect
[docs-badge]: https://img.shields.io/badge/docs-latest-blue.svg?style=flat-square
[docs]: https://docs.rs/trillium-redirect

A handler and `Conn` extension trait for HTTP redirects. Supports all redirect status codes
(301, 302, 303, 307, 308) with `302 Found` as the default.

## Example

```rust,no_run
use trillium_redirect::{Redirect, RedirectConnExt, RedirectStatus};

// Simple redirect with default 302 status
let handler = Redirect::to("/new-path");

// Permanent redirect
let handler = Redirect::to("/new-path")
    .with_redirect_status(RedirectStatus::MovedPermanently);

// Redirect from within a handler using the conn extension trait
let handler = |conn: trillium::Conn| async move {
    conn.redirect("/somewhere-else")
};
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
