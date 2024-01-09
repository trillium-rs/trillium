#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

/*!
trillium client is a http client that uses the same `conn` approach as
[`trillium`](https://trillium.rs) but which can be used
independently for any http client application.

## Connector

[`trillium_client::Client`] is built with a Connector. Each runtime crate
([`trillium_smol`](https://docs.trillium.rs/trillium_smol),
[`trillium_tokio`](https://docs.trillium.rs/trillium_tokio),
[`trillium_async_std`](https://docs.trillium.rs/trillium_tokio)) offers
a Connector implementation, which can optionally be combined with a
tls crate such as
[`trillium_rustls`](https://docs.trillium.rs/trillium_rustls) or
[`trillium_native_tls`](https://docs.trillium.rs/trillium_native_tls).

See the documentation for [`Client`] and [`Conn`] for further usage
examples.

*/

mod conn;
pub use conn::{Conn, UnexpectedStatusError, USER_AGENT};

#[cfg(feature = "json")]
pub use conn::ClientSerdeError;

mod pool;
// open an issue if you have a reason for pool to be public
pub(crate) use pool::Pool;

mod client;
pub use client::Client;

pub use trillium_http::{
    Body, Error, HeaderName, HeaderValue, HeaderValues, Headers, KnownHeaderName, Method, Result,
    Status, Version,
};

mod util;

pub use trillium_server_common::{async_trait, Connector, ObjectSafeConnector, Url};

/// constructs a new [`Client`] -- alias for [`Client::new`]
pub fn client(connector: impl Connector) -> Client {
    Client::new(connector)
}

mod into_url;
pub use into_url::IntoUrl;
