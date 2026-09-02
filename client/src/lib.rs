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
//! Each request picks its HTTP version by four rules:
//!
//! 1. **Reuse before establish, best protocol first.** A live pooled connection to the origin is
//!    used before a new one is opened, preferring HTTP/3, then HTTP/2, then HTTP/1.1.
//! 2. **New connections need prior knowledge for h3, and ALPN for h2.** A new connection uses
//!    HTTP/3 only when the origin is known to speak it: an `Http3` hint, an [`Alt-Svc`][altsvc]
//!    header from an earlier response, or an `alpn=h3` SVCB/HTTPS DNS record (see
//!    [Encrypted DNS](#encrypted-dns)). This requires a client built with
//!    [`Client::new_with_quic`]. Otherwise, over `https://` the server chooses h2 or h1.1 during
//!    the TLS handshake and the client uses whatever ALPN selected. Over `http://` the client
//!    speaks HTTP/1.1, unless h2 is hinted.
//! 3. **A hint is where to start, not where to stop.** If the hinted protocol can't be reached (an
//!    h3 endpoint that doesn't answer) or can't carry the request (an h2 or h3 peer without
//!    extended CONNECT for a websocket handshake), the client continues to the next protocol down.
//!    [`Conn::with_strict_http_version`] turns that continuation into an error.
//! 4. **The URL scheme never changes.** Continuing to an earlier protocol stays on the same scheme:
//!    h3 continues to h2 or h1.1 over TLS, and cleartext h2 continues to cleartext h1.1. Nothing is
//!    ever downgraded from TLS to cleartext.
//!
//! ```text
//!               ┌─ h3 known (hint, Alt-Svc, DNS) ─► QUIC ─ok─► HTTP/3 ─┐
//!               │                                    │fail             │ can't carry
//!   request ────┤                                    ▼                 │ the request
//!               ├─ pooled h2 ───────────────────► HTTP/2 ──────────────┤
//!               │                                    ▲                 │
//!               └─ new connection ── ALPN h2 ────────┘                 ▼
//!                       │     ALPN http/1.1, or cleartext        HTTP/1.1 (new
//!                       ▼                                          connection)
//!                    HTTP/1.1
//! ```
//!
//! Over `https://` with a TLS connector that doesn't surface ALPN selection
//! (`trillium_native_tls`), the client can't tell whether the server picked h2, so it uses h1.1
//! unless h2 is hinted. To opt out of h2 for every request on a client, remove it from the TLS
//! configuration's ALPN list (for example `RustlsConfig::without_http2()`).
//!
//! [altsvc]: https://datatracker.ietf.org/doc/html/rfc7838
//!
//! ### Version hints
//!
//! [`Conn::with_http_version`] names the protocol to try first. It also constrains the new
//! connection's ALPN to match, so the hint is honored over TLS rather than overridden by the
//! server's ALPN choice. The [`http_version`](Conn::http_version) accessor reports the unset
//! default as [`Version::Http1_1`]. Hints are per-[`Conn`]; mix them freely on requests sharing
//! one [`Client`].
//!
//! | hint | behavior | curl equivalent |
//! |---|---|---|
//! | `Version::Http3` | Dial QUIC directly, skipping the Alt-Svc cache. Continues to h2 / h1.1 if the QUIC connection fails. | `--http3` |
//! | `Version::Http2` over `https` | TLS handshake advertising only `h2`, then the h2 preface without checking ALPN. Works with TLS connectors that don't surface ALPN. A server that doesn't speak h2 surfaces as an IO error: the preface commits the connection. | `--http2-prior-knowledge` |
//! | `Version::Http2` over `http` | Cleartext h2 (h2c) preface. Same commitment as above. | `--http2-prior-knowledge` |
//! | `Version::Http1_1` | HTTP/1.1 only: no h3, no h2. | `--http1.1` |
//! | `Version::Http1_0` | HTTP/1.0 wire format (no `Host`, no chunked encoding). | `--http1.0` |
//! | _unset_ | Rules 1 and 2 above. | (default) |
//!
//! ### Strict mode
//!
//! [`Conn::with_strict_http_version`] (or [`Client::with_strict_http_version`] for every conn)
//! makes a request fail when the protocol it was matched to can't carry it, instead of
//! continuing to an earlier protocol. Off by default. It applies to the websocket handshake
//! below; the h2 prior-knowledge commitment and the h3 connection-failure continuation are the
//! same either way.
//!
//! ## WebSockets and WebTransport
//!
//! With the `websockets` cargo feature, `Conn::into_websocket` performs a websocket handshake
//! and returns a `WebSocketConn`. Over HTTP/1.1 this is the RFC 6455 `Upgrade` handshake; over
//! HTTP/2 and HTTP/3 it is an extended CONNECT (RFC 8441, RFC 9220). The version follows the
//! rules above: a server that speaks h2 or h3 but does not advertise extended CONNECT is retried
//! as an HTTP/1.1 upgrade on a new connection, or fails under strict mode. With the
//! `webtransport` cargo feature, `Client::webtransport(url)` + `Conn::into_webtransport()`
//! open a multiplexed WebTransport-over-h3 session (RFC 9220 +
//! draft-ietf-webtrans-http3); WebTransport exists only on HTTP/3, so those conns are strict.
//! Multiple WebTransport sessions to the same origin coalesce
//! onto a single underlying QUIC connection — see the `webtransport` module for details.
//!
//! ## Server-Sent Events
//!
//! With the `sse` cargo feature, [`Conn::into_sse`](sse) executes a request and reads the
//! response body as a `text/event-stream`, returning an [`EventStream`] — a [`Stream`] of
//! [`Event`]s parsed per the [SSE specification][sse-spec]. Unlike the WebSocket and WebTransport
//! upgrades, SSE is not a protocol switch: an event stream is an ordinary response whose body is
//! read incrementally, so it works the same over HTTP/1.x, HTTP/2, and HTTP/3. This is a
//! single-response stream — it ends when the connection closes and does not implement the
//! [`EventSource`][es] automatic-reconnection behavior. See the [`sse`] module for details.
//!
//! [`Stream`]: https://docs.rs/futures-core/latest/futures_core/stream/trait.Stream.html
//! [sse-spec]: https://html.spec.whatwg.org/multipage/server-sent-events.html
//! [es]: https://developer.mozilla.org/en-US/docs/Web/API/EventSource
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
mod reaper;
mod response_body;
#[cfg(feature = "sse")]
pub mod sse;
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
#[cfg(feature = "sse")]
pub use sse::{Event, EventStream, SseError, SseErrorKind};
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
