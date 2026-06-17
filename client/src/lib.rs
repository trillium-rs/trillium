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

//! trillium client is an HTTP client that uses the same `conn` approach as
//! [`trillium`](https://trillium.rs) but which can be used
//! independently for any HTTP client application.
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
//!   (`trillium_native_tls`): h1 only by default, since trillium can't tell whether the server
//!   picked h2. Use the `Version::Http2` hint described below to force h2 over TLS in that case.
//! - Over `https://` when the [`Client`] was built with
//!   [`Client::new_with_quic`](Client::new_with_quic): the client may use h3 for origins that have
//!   advertised it via [`Alt-Svc`][altsvc], that publish an `alpn=h3` SVCB/HTTPS DNS record (when
//!   an encrypted resolver is configured — see [Encrypted DNS](#encrypted-dns)), or that the user
//!   has hinted (see below).
//! - Over `http://`: h1 only. There is no h2c probing without explicit prior knowledge.
//!
//! [altsvc]: https://datatracker.ietf.org/doc/html/rfc7838
//!
//! ### Prior-knowledge hints
//!
//! Setting [`Conn::http_version`](Conn::with_http_version) before sending the request
//! signals **prior knowledge** of what the server speaks. By default no hint is set, which means
//! "use auto-discovery." Setting any explicit version **pins** the protocol and suppresses
//! auto-discovery — no Alt-Svc h3, no ALPN/pooled h2 promotion — and constrains the connection's
//! ALPN to match (an h1 pin advertises only `http/1.1`, an h2 pin only `h2`), so the pin is honored
//! over TLS rather than overridden by ALPN. The [`http_version`](Conn::http_version) accessor
//! reports the unset default as [`Version::Http1_1`].
//!
//! | hint | URL scheme | behavior | curl equivalent |
//! |---|---|---|---|
//! | `Version::Http3` | `https` | Skip the [`Alt-Svc`][altsvc] cache and dial QUIC directly. Falls back to auto-discovery (h2 / h1) if QUIC connect fails. Requires [`Client::new_with_quic`](Client::new_with_quic). | `--http3` |
//! | `Version::Http2` | `https` | TLS handshake advertising only `h2` in ALPN, then start the h2 driver immediately without checking the negotiated ALPN. **No fallback** — a non-h2-speaking server surfaces as an IO error. Also works with TLS connectors that don't surface ALPN selection. | (curl bundles this with `--http2-prior-knowledge`'s cleartext mode) |
//! | `Version::Http2` | `http` | h2c immediate preface (cleartext h2 prior knowledge). **No fallback**. | `--http2-prior-knowledge` |
//! | `Version::Http1_1` | any | Force HTTP/1.1: no h3 Alt-Svc, no h2 ALPN/pool promotion. | `--http1.1` |
//! | `Version::Http1_0` | any | h1.0 wire format (no `Host`, no chunked encoding, etc.). | `--http1.0` |
//! | _unset_ (default) | any | Auto-discovery as described above. | (default) |
//!
//! Hints are per-[`Conn`]; mix them freely on requests sharing one [`Client`].
//!
//! ### Forcing h1.1
//!
//! Set the [`Version::Http1_1`] hint on the request — the per-request equivalent of curl's
//! `--http1.1`. It pins HTTP/1.1 even when the connector would otherwise negotiate h2 via ALPN or
//! use h3 via Alt-Svc, by advertising only `http/1.1` in this connection's ALPN. (Over
//! `trillium_native_tls`, which doesn't yet honor per-connection ALPN, the pin still skips h2/h3
//! promotion but can't constrain the handshake — in practice harmless, since native-tls advertises
//! no ALPN by default.) To opt out of h2 ALPN advertisement at the connection level for *all*
//! requests on a client, that remains a TLS configuration concern: use
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
//!
//! ## Encrypted DNS
//!
//! With the `hickory` cargo feature, the client can route all of its DNS through an encrypted
//! resolver of your choice rather than sending plaintext queries to the operating system's
//! resolver. `Client::with_doh` uses DNS-over-HTTPS ([RFC 8484]), `Client::with_dot` DNS-over-TLS
//! ([RFC 7858]), and `Client::with_doq` DNS-over-QUIC ([RFC 9250]); a client uses at most one, and
//! a later call replaces an earlier one. DoH lookups ride the client's own connection pool, so they
//! reuse and multiplex like any other request. A single resolution is cached and shared across
//! HTTP/1, HTTP/2, and HTTP/3.
//!
//! Resolution is fail-closed: once a resolver is configured, a lookup it can't answer fails the
//! request rather than falling back to the system resolver, so a query never leaks to a (possibly
//! plaintext) local resolver. The resolver's own host is the one exception — it's resolved once via
//! the underlying connector to bootstrap the connection; give the resolver as an IP address to skip
//! even that.
//!
//! SVCB and HTTPS DNS records ([RFC 9460]) are fetched too, letting a server advertise HTTP/3
//! support directly in DNS. A domain publishing `alpn=h3` is reached over HTTP/3 on the first
//! request by an HTTP/3-capable client ([`Client::new_with_quic`]), with no [`Alt-Svc`][altsvc]
//! round-trip. The connection to a DoH resolver itself negotiates h1/h2 by default;
//! `Client::with_doh3` pins it to HTTP/3 for resolvers that serve DoH over HTTP/3 without
//! advertising it. `with_dot` requires a TLS connector and `with_doq` an HTTP/3-capable client.
//!
//! [RFC 8484]: https://www.rfc-editor.org/rfc/rfc8484
//! [RFC 7858]: https://www.rfc-editor.org/rfc/rfc7858
//! [RFC 9250]: https://www.rfc-editor.org/rfc/rfc9250
//! [RFC 9460]: https://www.rfc-editor.org/rfc/rfc9460

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}
mod client;
mod client_handler;
mod conn;
mod conn_handler_ext;
#[cfg(feature = "hickory")]
mod dns;
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
