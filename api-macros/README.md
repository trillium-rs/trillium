# trillium-api-macros

Proc-macro derives for [`trillium-api`](https://docs.rs/trillium-api).

Provides `#[derive(TryFromConn)]` and `#[derive(Handler)]` configured by an `#[api(...)]` attribute, so that user types can act as extractors and/or handlers without hand-written impls.

```rust,ignore
use trillium_api::{TryFromConn, Handler};

#[derive(Clone, TryFromConn, Handler)]
#[api(state, clone)]
struct CurrentUser { /* ... */ }
```

See the `trillium-api` crate docs for usage. This crate is re-exported from `trillium-api`; depend on `trillium-api` rather than this crate directly.
