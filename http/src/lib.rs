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
#![allow(clippy::must_use_candidate, clippy::module_name_repetitions)]
//! This crate provides the http 1.x implementation for Trillium.
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

mod received_body;
pub use received_body::ReceivedBody;
#[cfg(feature = "unstable")]
pub use received_body::ReceivedBodyState;

mod error;
pub use error::{Error, Result};

mod conn;
pub use conn::{Conn, SERVER};

mod connection_status;
pub use connection_status::ConnectionStatus;

mod synthetic;
pub use synthetic::Synthetic;

mod upgrade;
pub use swansong::Swansong;
pub use upgrade::Upgrade;

mod mut_cow;
pub(crate) use mut_cow::MutCow;

mod util;

mod body;
pub use body::Body;
pub use type_set::{self, TypeSet};

mod headers;
pub use headers::{HeaderName, HeaderValue, HeaderValues, Headers, KnownHeaderName};

mod status;
pub use status::Status;

mod method;
pub use method::Method;

mod version;
pub use version::Version;

/// Types to represent the bidirectional data stream over which the
/// HTTP protocol is communicated
pub mod transport;

/// A pre-rendered http response to send when the server is at capacity.
pub const SERVICE_UNAVAILABLE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r
Connection: close\r
Content-Length: 0\r
Retry-After: 60\r
\r\n";

#[cfg(feature = "http-compat-1")]
pub mod http_compat1;

#[cfg(feature = "http-compat")]
pub mod http_compat0;

#[cfg(feature = "http-compat")]
pub use http_compat0 as http_compat; // for semver

mod bufwriter;
pub(crate) use bufwriter::BufWriter;

mod http_config;
pub use http_config::HttpConfig;

pub(crate) mod after_send;

mod buffer;
#[cfg(feature = "unstable")]
pub use buffer::Buffer;
#[cfg(not(feature = "unstable"))]
pub(crate) use buffer::Buffer;

mod copy;
#[cfg(feature = "unstable")]
pub use copy::copy;
#[cfg(not(feature = "unstable"))]
pub(crate) use copy::copy;

mod liveness;
mod server_config;
pub use server_config::ServerConfig;
