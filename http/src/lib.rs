#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    nonstandard_style,
    unused_qualifications
)]
#![warn(
    missing_docs,
    clippy::pedantic,
    clippy::perf,
    clippy::cargo,
    rustdoc::broken_intra_doc_links
)]
#![allow(
    clippy::must_use_candidate,
    clippy::module_name_repetitions,
    clippy::multiple_crate_versions
)]
//! This crate provides the http implementations for Trillium.
//!
//! ## Stability
//!
//! As this is primarily intended for internal use by the [Trillium
//! crate](https://docs.trillium.rs/trillium), the api is likely to be
//! less stable than that of the higher level abstractions in Trillium.
//!
//! ## Cargo features
//!
//! All are off by default.
//!
//! - **`serde`** — implements `serde::Serialize` for [`Headers`], [`HeaderName`], [`HeaderValue`],
//!   and [`HeaderValues`], and both `Serialize` and `Deserialize` for [`Method`], [`Status`], and
//!   [`Version`]. The header types are serialize-only — suited to logging and inspection rather
//!   than a faithful round-trip; reach for `rkyv_08` when you need to read the data back.
//! - **`rkyv_08`** — implements [rkyv](https://docs.rs/rkyv) 0.8's `Archive`, `Serialize`, and
//!   `Deserialize` for [`Method`], [`Status`], [`Version`], [`HeaderName`], [`HeaderValue`],
//!   [`HeaderValues`], and [`Headers`], so they round-trip losslessly through `rkyv::to_bytes` /
//!   `rkyv::from_bytes` — non-utf8 header values and repeated values included. Suited to durable
//!   binary persistence such as an on-disk response cache.
//! - **`http-compat-0`** and **`http-compat-1`** — conversions between the core trillium-http types
//!   and those of the [`http`](https://docs.rs/http) crate, at its 0.x and 1.x releases
//!   respectively. Enable both to interoperate with each.
//!
//! ## Protocol dispatch
//!
//! trillium-http supports HTTP/1.0, HTTP/1.1, HTTP/2, and (via `trillium-quinn`)
//! HTTP/3 on the same listener. The version that a given connection speaks is
//! decided at accept time:
//!
//! | Listener | ALPN result | First bytes | Protocol |
//! |---|---|---|---|
//! | TCP + TLS | `h2` | — | HTTP/2 over TLS |
//! | TCP + TLS | `http/1.1` | — | HTTP/1.1 over TLS |
//! | TCP + TLS | absent or other | match HTTP/2 preface (`PRI * HTTP/2.0…`) | HTTP/2 "prior knowledge" over TLS |
//! | TCP + TLS | absent or other | anything else | HTTP/1.1 over TLS |
//! | TCP, cleartext | — | match HTTP/2 preface | HTTP/2 "prior knowledge" (h2c) |
//! | TCP, cleartext | — | anything else | HTTP/1.x |
//! | QUIC | — | — | HTTP/3 |
//!
//! h2c via the HTTP/1.1 `Upgrade` mechanism (RFC 7540 §3.2, removed in RFC 9113)
//! is **not** supported — if an `Upgrade: h2c` header arrives on an h1 request it
//! is logged and ignored.
//!
//! The TLS acceptors shipped with trillium that advertise ALPN (`trillium-rustls` and
//! `trillium-openssl`) advertise `h2, http/1.1` automatically. `trillium-native-tls`
//! does not surface ALPN. Users with custom TLS configs are responsible for advertising
//! `h2` themselves if h2 is desired. Clients on TLS stacks that don't expose an ALPN
//! knob can still reach h2 via the prior-knowledge preface path — ALPN comes back
//! absent and the server peeks the first 24 bytes.
//!
//! All h2/h3-specific tuning flows through [`HttpConfig`] — see its field
//! documentation for the full list of knobs (stream / connection flow-control
//! windows, max concurrent streams, max frame size, etc.).
//!
//! ## Example
//!
//! This is an elaborate example that demonstrates some of `trillium_http`'s
//! capabilities.  Please note that trillium itself provides a much more
//! usable interface on top of `trillium_http`, at very little cost.
//!
//! ```
//! fn main() -> trillium_http::Result<()> {
//!     smol::block_on(async {
//!         use async_net::TcpListener;
//!         use futures_lite::StreamExt;
//!         use std::sync::Arc;
//!         use trillium_http::HttpContext;
//!
//!         let context = Arc::new(HttpContext::default());
//!         let listener = TcpListener::bind(("localhost", 0)).await?;
//!         let local_addr = listener.local_addr().unwrap();
//!         let server_handle = smol::spawn({
//!             let context = context.clone();
//!             async move {
//!                 let mut incoming = context.swansong().interrupt(listener.incoming());
//!
//!                 while let Some(Ok(stream)) = incoming.next().await {
//!                     smol::spawn(context.clone().run(stream, |mut conn| async move {
//!                         conn.set_response_body("hello world");
//!                         conn.set_status(200);
//!                         conn
//!                     }))
//!                     .detach()
//!                 }
//!             }
//!         });
//!
//!         // this example uses the trillium client
//!         // any other http client would work here too
//!         let client = trillium_client::Client::new(trillium_smol::ClientConfig::default())
//!             .with_base(local_addr);
//!         let mut client_conn = client.get("/").await?;
//!
//!         assert_eq!(client_conn.status().unwrap(), 200);
//!         assert_eq!(
//!             client_conn.response_headers().get_str("content-length"),
//!             Some("11")
//!         );
//!         assert_eq!(
//!             client_conn.response_body().read_string().await?,
//!             "hello world"
//!         );
//!
//!         context.shut_down().await; // stop the server after one request
//!         server_handle.await; // wait for the server to shut down
//!         Ok(())
//!     })
//! }
//! ```

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

pub(crate) mod after_send;
mod body;
mod buffer;
mod bufwriter;
mod conn;
mod connection_status;
mod copy;
mod error;
pub mod h2;
pub mod h3;
pub mod headers;
#[cfg(any(feature = "http-compat-0", feature = "http-compat-1"))]
mod http_compat;
#[cfg(feature = "http-compat-0")]
pub use http_compat::http_compat0;
#[cfg(feature = "http-compat-1")]
pub use http_compat::http_compat1;
mod http_config;
mod http_context;
mod liveness;
mod method;
mod mut_cow;
mod priority;
mod protocol_session;
mod received_body;
#[cfg(feature = "rkyv_08")]
mod rkyv_08;
mod status;
mod synthetic;
mod upgrade;
mod util;
mod version;

#[cfg(feature = "unstable")]
#[doc(hidden)]
pub use body::BodyFraming;
pub use body::{Body, BodySource};
#[cfg(feature = "unstable")]
#[doc(hidden)]
pub use buffer::Buffer;
#[cfg(not(feature = "unstable"))]
pub(crate) use buffer::Buffer;
#[cfg(feature = "unstable")]
pub use bufwriter::BufWriter;
#[cfg(not(feature = "unstable"))]
pub(crate) use bufwriter::BufWriter;
pub use conn::Conn;
pub use connection_status::ConnectionStatus;
#[cfg(feature = "unstable")]
#[doc(hidden)]
pub use copy::copy;
#[cfg(not(feature = "unstable"))]
pub(crate) use copy::copy;
pub use error::{Error, Result};
pub use headers::{HeaderName, HeaderValue, HeaderValues, Headers, KnownHeaderName, SERVER_HEADER};
pub use http_config::HttpConfig;
#[cfg(feature = "unstable")]
pub use http_context::parse_head_for_bench;
pub use http_context::{HttpContext, run_with_initial_bytes};
pub use method::Method;
#[cfg(feature = "unstable")]
#[doc(hidden)]
pub use mut_cow::MutCow;
#[cfg(not(feature = "unstable"))]
pub(crate) use mut_cow::MutCow;
pub use priority::Priority;
#[cfg(feature = "unstable")]
pub use protocol_session::ProtocolSession;
#[cfg(not(feature = "unstable"))]
pub(crate) use protocol_session::ProtocolSession;
pub use received_body::ReceivedBody;
#[cfg(feature = "unstable")]
#[doc(hidden)]
pub use received_body::{H3BodyFrameType, ReceivedBodyState};
pub use status::Status;
pub use swansong::Swansong;
#[doc(hidden)]
pub use synthetic::Synthetic;
pub use type_set::{self, TypeSet};
pub use upgrade::Upgrade;
#[cfg(feature = "unstable")]
#[doc(hidden)]
pub use util::validate_content_length;
pub use version::Version;

/// A pre-rendered http response to send when the server is at capacity.
pub const SERVICE_UNAVAILABLE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r
Connection: close\r
Content-Length: 0\r
Retry-After: 60\r
\r\n";

/// The version of this crate
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
