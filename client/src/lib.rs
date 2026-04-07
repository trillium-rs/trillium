#![cfg_attr(docsrs, feature(doc_cfg))]
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

//! trillium client is a http client that uses the same `conn` approach as
//! [`trillium`](https://trillium.rs) but which can be used
//! independently for any http client application.
//!
//! ## Connector
//!
//! [`trillium_client::Client`](Client) is built with a [`Connector`]. Each runtime crate
//! ([`trillium_smol`](https://docs.trillium.rs/trillium_smol),
//! [`trillium_tokio`](https://docs.trillium.rs/trillium_tokio),
//! [`trillium_async_std`](https://docs.trillium.rs/trillium_async_std)) offers
//! a Connector implementation, which can optionally be combined with a
//! tls crate such as
//! [`trillium_rustls`](https://docs.trillium.rs/trillium_rustls) or
//! [`trillium_native_tls`](https://docs.trillium.rs/trillium_native_tls).
//!
//! See the documentation for [`Client`] and [`Conn`] for further usage
//! examples.

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}
mod client;
mod conn;
mod h3;
mod into_url;
mod pool;
mod response_body;
mod util;
#[cfg(feature = "websockets")]
pub mod websocket;

pub use client::Client;
#[cfg(any(feature = "serde_json", feature = "sonic-rs"))]
pub use conn::ClientSerdeError;
pub use conn::{Conn, USER_AGENT, UnexpectedStatusError};
pub use into_url::IntoUrl;
// open an issue if you have a reason for pool to be public
pub(crate) use pool::Pool;
pub use response_body::ResponseBody;
pub use trillium_http::{
    Body, Error, HeaderName, HeaderValue, HeaderValues, Headers, KnownHeaderName, Method, Result,
    Status, Version,
};
pub use trillium_server_common::{
    ArcedConnector, ArcedQuicClientConfig, Connector, QuicClientConfig, Url,
};
#[cfg(feature = "websockets")]
pub use trillium_websockets::{WebSocketConfig, WebSocketConn, async_tungstenite, tungstenite};
#[cfg(feature = "websockets")]
pub use websocket::WebSocketUpgradeError;

#[cfg(all(feature = "serde_json", feature = "sonic-rs"))]
compile_error!("cargo features \"serde_json\" and \"sonic-rs\" are mutually exclusive");

#[cfg(feature = "serde_json")]
#[cfg_attr(docsrs, doc(cfg(feature = "serde_json")))]
pub use serde_json::{Value, json};
#[cfg(feature = "sonic-rs")]
#[cfg_attr(docsrs, doc(cfg(feature = "sonic-rs")))]
pub use sonic_rs::{Value, json};

/// constructs a new [`Client`] -- alias for [`Client::new`]
pub fn client(connector: impl Connector) -> Client {
    Client::new(connector)
}
