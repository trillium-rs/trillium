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
trillium.

this was primarily built for the
[`trillium_proxy`](https://docs.trillium.rs/trillium_proxy/) crate,
but might end up fitting well into trillium apps for other purposes.

In order to use http keep-alive connection pooling, make requests from
a [`trillium_client::Client`](Client). To make a one-off request,
build a [`trillium_client::Conn`](Conn) directly. Please note that a
trillium_client Conn, while conceptually similar, is different from
trillium::Conn and trillium_http::Conn.

## Connector

[`Client`] and [`Conn`] are generic over an implementation of
[`Connector`]. Each runtime crate ([`trillium_smol`](https://docs.trillium.rs/trillium_smol),
[`trillium_tokio`](https://docs.trillium.rs/trillium_tokio),
[`trillium_async_std`](https://docs.trillium.rs/trillium_tokio)) offers
a Connector implementation, which can optionally be combined with a
tls crate ([`trillium_rustls`](https://docs.trillium.rs/trillium_rustls) and
[`trillium_native_tls`](https://docs.trillium.rs/trillium_native_tls)
each offer Connector wrappers.

See the documentation for [`Client`] and [`Conn`] for further usage
examples.

*/

mod conn;
pub use conn::Conn;

#[cfg(feature = "json")]
pub use conn::ClientSerdeError;

mod pool;
// open an issue if you have a reason for pool to be public
pub(crate) use pool::Pool;

mod client;
pub use client::Client;

pub use trillium_http::{Error, Result};

mod util;

pub use trillium_server_common::Connector;

mod client_like;
pub use client_like::ClientLike;

/// constructs a new [`Client`] -- alias for [`Client::new`]
pub fn client(connector: impl Connector) -> Client {
    Client::new(connector)
}
