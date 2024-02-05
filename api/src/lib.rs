/*!
This crate provides handlers for common http api behavior.

Eventually, some of this crate may move into the trillium crate, but
for now it exists separately for ease of iteration. Expect more
breaking changes in this crate then in the trillium crate.

## Formats supported:

Currently, this crate supports *receiving* `application/json` and
`application/x-form-www-urlencoded` by default. To disable
`application/x-form-www-urlencoded` support, use `default-features =
false`.

This crate currently only supports sending json responses, but may
eventually add `Accepts` negotiation and further outbound response
content types.

The [`ApiConnExt`] extension trait and [`ApiHandler`] can be used
independently or in combination.

[`ApiHandler`] provides a different and more experimental interface to writing trillium handlers,
with different performance and ergonomic considerations.
*/
#![forbid(unsafe_code)]
#![deny(
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

mod api_conn_ext;
mod api_handler;
mod before_send;
mod body;
mod cancel_on_disconnect;
mod error;
mod from_conn;
mod halt;
mod json;
mod state;
mod try_from_conn;

pub use api_conn_ext::ApiConnExt;
pub use api_handler::{api, ApiHandler};
pub use before_send::BeforeSend;
pub use body::Body;
pub use cancel_on_disconnect::{cancel_on_disconnect, CancelOnDisconnect};
pub use error::Error;
pub use from_conn::FromConn;
pub use halt::Halt;
pub use json::Json;
pub use serde_json::{json, Value};
pub use state::State;
pub use try_from_conn::TryFromConn;

/// trait alias for a result with this crate's [`Error`]
pub type Result<T> = std::result::Result<T, Error>;
