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
//! [`trillium_rustls`](https://docs.trillium.rs/trillium_rustls),
//! [`trillium_native_tls`](https://docs.trillium.rs/trillium_native_tls), or
//! [`trillium_openssl`](https://docs.trillium.rs/trillium_openssl).
//!
//! See the documentation for [`Client`] and [`Conn`] for further usage
//! examples.
//!
//! ## Protocol selection
//!
//! By default, trillium-client auto-discovers the best HTTP version for each request:
//!
//! - Over `https://` with a TLS connector that advertises `h2` in ALPN *and* exposes the server's selection
//!   back to trillium (the default for [`trillium_rustls::RustlsConfig`](https://docs.trillium.rs/trillium_rustls/struct.RustlsConfig.html)
//!   and [`trillium_openssl::OpenSslConfig`](https://docs.trillium.rs/trillium_openssl/struct.OpenSslConfig.html)):
//!   the server picks h2 or h1.1 during the TLS handshake. Whatever ALPN selects is what the client
//!   uses.
//! - Over `https://` with `h2` removed from the ALPN list (e.g. `RustlsConfig::without_http2()`):
//!   h1 only.
//! - Over `https://` with a TLS connector that doesn't surface ALPN selection
//!   (`trillium_native_tls` at time of writing): h1 only by default, since trillium can't tell
//!   whether the server picked h2. Use the `Version::Http2` hint described below to force h2 over
//!   TLS in that case.
//! - Over `https://` when the [`Client`] was built with
//!   [`Client::new_with_quic`](Client::new_with_quic): the client may use h3 for origins that have
//!   advertised it via [`Alt-Svc`][altsvc] or that the user has hinted (see below).
//! - Over `http://`: h1 only. There is no h2c probing without explicit prior knowledge.
//!
//! [altsvc]: https://datatracker.ietf.org/doc/html/rfc7838
//!
//! ### Prior-knowledge hints
//!
//! Setting [`Conn::http_version`](Conn::with_http_version) before sending the request
//! signals **prior knowledge** of what the server speaks. The default value is
//! [`Version::Http1_1`], which means "no hint — use auto-discovery."
//!
//! | hint | URL scheme | behavior | curl equivalent |
//! |---|---|---|---|
//! | `Version::Http3` | `https` | Skip the [`Alt-Svc`][altsvc] cache and dial QUIC directly. Falls back to h2 / h1 if QUIC connect fails. Requires [`Client::new_with_quic`](Client::new_with_quic). | `--http3` |
//! | `Version::Http2` | `https` | TLS handshake (with whatever ALPN the connector advertises), then start the h2 driver immediately without checking the negotiated ALPN. **No fallback** — a non-h2-speaking server surfaces as an IO error. Useful with TLS connectors that don't surface ALPN selection. | (curl bundles this with `--http2-prior-knowledge`'s cleartext mode) |
//! | `Version::Http2` | `http` | h2c immediate preface (cleartext h2 prior knowledge). **No fallback**. | `--http2-prior-knowledge` |
//! | `Version::Http1_1` (default) | any | Auto-discovery as described above. | (default) |
//! | `Version::Http1_0` | any | h1.0 wire format (no `Host`, no chunked encoding, etc.). | `--http1.0` |
//!
//! Hints are per-[`Conn`]; mix them freely on requests sharing one [`Client`].
//!
//! ### Forcing h1.1 (no h2 ALPN)
//!
//! There is no per-request knob equivalent to curl's `--http1.1`. Opting out of h2 ALPN
//! advertisement is a TLS configuration concern, not a per-request concern: use
//! [`RustlsConfig::without_http2()`](https://docs.trillium.rs/trillium_rustls/struct.RustlsConfig.html#method.without_http2)
//! (or the equivalent on whichever TLS crate you're using) when constructing the
//! [`Client`].
//!
//! ## WebSockets and WebTransport
//!
//! With the `websockets` cargo feature, `Conn::into_websocket` transforms a built conn into
//! a `WebSocketConn` (RFC 6455 over h1, RFC 8441 extended CONNECT over h2). With the
//! `webtransport` cargo feature, `Client::webtransport(url)` + `Conn::into_webtransport()`
//! open a multiplexed WebTransport-over-h3 session (RFC 9220 +
//! draft-ietf-webtrans-http3). Multiple WebTransport sessions to the same origin coalesce
//! onto a single underlying QUIC connection — see the `webtransport` module for details.

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}
mod client;
mod client_handler;
mod conn;
mod conn_handler_ext;
mod h3;
mod into_url;
mod pool;
mod response_body;
mod util;
#[cfg(feature = "websockets")]
pub mod websocket;
#[cfg(feature = "webtransport")]
pub mod webtransport;

pub use client::Client;
pub use client_handler::ClientHandler;
#[cfg(any(feature = "serde_json", feature = "sonic-rs"))]
pub use conn::ClientSerdeError;
pub use conn::{Conn, USER_AGENT, UnexpectedStatusError};
pub use conn_handler_ext::ConnExt;
pub use into_url::IntoUrl;
// open an issue if you have a reason for pool to be public
pub(crate) use pool::Pool;
pub use response_body::ResponseBody;
pub use trillium_http::{
    Body, BodySource, Error, HeaderName, HeaderValue, HeaderValues, Headers, KnownHeaderName,
    Method, Result, Status, Version,
};
pub use trillium_server_common::{
    ArcedConnector, ArcedQuicClientConfig, Connector, QuicClientConfig, Url, url,
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
