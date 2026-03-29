#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    nonstandard_style,
    unused_qualifications
)]
#![warn(missing_docs, clippy::pedantic, clippy::perf, clippy::cargo)]
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
//!         use trillium_http::ServerConfig;
//!
//!         let server_config = Arc::new(ServerConfig::default());
//!         let listener = TcpListener::bind(("localhost", 0)).await?;
//!         let local_addr = listener.local_addr().unwrap();
//!         let server_handle = smol::spawn({
//!             let server_config = server_config.clone();
//!             async move {
//!                 let mut incoming = server_config.swansong().interrupt(listener.incoming());
//!
//!                 while let Some(Ok(stream)) = incoming.next().await {
//!                     smol::spawn(server_config.clone().run(stream, |mut conn| async move {
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
//!         server_config.shut_down().await; // stop the server after one request
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
pub mod h3;
pub mod headers;
#[cfg(feature = "http-compat")]
pub mod http_compat0;
#[cfg(feature = "http-compat-1")]
pub mod http_compat1;
mod http_config;
mod liveness;
mod method;
mod mut_cow;
mod received_body;
mod server_config;
mod status;
mod synthetic;
mod upgrade;
mod util;
mod version;

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
pub use conn::{Conn, SERVER};
pub use connection_status::ConnectionStatus;
#[cfg(feature = "unstable")]
#[doc(hidden)]
pub use copy::copy;
#[cfg(not(feature = "unstable"))]
pub(crate) use copy::copy;
pub use error::{Error, Result};
pub use headers::{HeaderName, HeaderValue, HeaderValues, Headers, KnownHeaderName};
pub use http_config::HttpConfig;
pub use method::Method;
pub(crate) use mut_cow::MutCow;
pub use received_body::ReceivedBody;
#[cfg(feature = "unstable")]
#[doc(hidden)]
pub use received_body::{H3BodyFrameType, ReceivedBodyState};
pub use server_config::ServerConfig;
pub use status::Status;
pub use swansong::Swansong;
#[doc(hidden)]
pub use synthetic::Synthetic;
pub use type_set::{self, TypeSet};
pub use upgrade::Upgrade;
pub use version::Version;

/// A pre-rendered http response to send when the server is at capacity.
pub const SERVICE_UNAVAILABLE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r
Connection: close\r
Content-Length: 0\r
Retry-After: 60\r
\r\n";
