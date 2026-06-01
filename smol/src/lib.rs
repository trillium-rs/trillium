#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

//! # Trillium adapter using smol and async-global-executor
//!
//! ## Default / 12-factor applications
//!
//! ```rust,no_run
//! trillium_smol::run(|conn: trillium::Conn| async move { conn.ok("hello smol") });
//! ```
//!
//! ## Server configuration
//!
//! For more details, see [trillium_smol::config](crate::config).
//!
//! ```rust
//! let swansong = trillium_smol::Swansong::new();
//! # swansong.shut_down(); // stoppping the server immediately for the test
//! trillium_smol::config()
//!     .with_port(0)
//!     .with_host("127.0.0.1")
//!     .without_signals()
//!     .with_nodelay()
//!     .with_acceptor(()) // see [`trillium_rustls`], [`trillium_native_tls`], and [`trillium_openssl`]
//!     .with_swansong(swansong)
//!     .run(|conn: trillium::Conn| async move { conn.ok("hello smol") });
//! ```
//!
//! For advanced binding — several listeners on one server, fallible binds you
//! can recover from, or adopting an already-bound socket — call `.listeners()`
//! to get a [`ListenerConfig`].
//!
//! ## Thread-per-core with `SO_REUSEPORT` on Linux
//!
//! `SO_REUSEPORT` is a socket option that lets several sockets bind the same
//! address and port at once, with the kernel distributing incoming connections
//! across them. Enable the `reuseport` cargo feature on Linux to use it for
//! thread-per-core fan-out:
//!
//! ```rust,ignore
//! use trillium::Conn;
//!
//! fn main() -> std::io::Result<()> {
//!     trillium_smol::config()
//!         .listeners()
//!         .bind_reuseport_tcp(8080)?
//!         .run(|conn: Conn| async move { conn.ok("hello") });
//!     Ok(())
//! }
//! ```
//!
//! Each worker thread runs its own single-threaded executor, pinned to a core,
//! driving that worker's accept loop — one `SO_REUSEPORT` listener per worker,
//! and every connection it accepts is handled on that same executor. The shared
//! multi-threaded global executor is still present alongside them, hosting
//! HTTP/3, signal handling, and the application tasks you spawn, so QUIC is
//! never fanned out this way. Set the worker count with
//! `.with_reuseport_workers(n)`; it defaults to the `WORKERS` environment
//! variable, or if that's not set, to available parallelism.
//!
//! This trades the global executor's load balancing for per-core locality,
//! which can improve throughput for short, CPU-cheap requests served over many
//! connections. It is gated off on platforms where plain `SO_REUSEPORT` does
//! not distribute connections (including macOS), where it would offer no
//! benefit.
//!
//! ## Client
//!
//! ```rust
//! # #[cfg(feature = "smol")]
//! trillium_testing::with_server("ok", |url| async move {
//!     use trillium_client::{Client, Conn};
//!     use trillium_smol::TcpConnector;
//!     let mut conn = Conn::<TcpConnector>::get(url.clone()).execute().await?;
//!     assert_eq!(conn.response_body().read_string().await?, "ok");
//!
//!     let client = Client::<TcpConnector>::new();
//!     let mut conn = client.get(url);
//!     conn.send().await?;
//!     assert_eq!(conn.response_body().read_string().await?, "ok");
//!     Ok(())
//! });
//! ```

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

use trillium::Handler;
pub use trillium_server_common::{
    Binding, Connector, IntoListenAddr, ListenerConfig, Runtime, RuntimeTrait, Swansong,
};

mod client;
pub use client::ClientConfig;

mod server;
use server::Config;

mod transport;
pub use async_global_executor;
pub use async_io;
pub use async_net;
pub use transport::SmolTransport;

mod runtime;
pub use runtime::SmolRuntime;

#[cfg(all(
    feature = "reuseport",
    unix,
    not(target_os = "solaris"),
    not(target_os = "illumos"),
    not(target_os = "cygwin"),
    not(target_vendor = "apple")
))]
mod reuseport;

mod udp;
pub use udp::SmolUdpSocket;

/// # Runs a trillium handler in a sync context with default config
///
/// Runs a trillium handler on the async-global-executor runtime with
/// default configuration. See [`crate::config`] for what the defaults are
/// and how to override them
///
///
/// This function will block the current thread until the server shuts
/// down
pub fn run(handler: impl Handler) {
    config().run(handler)
}

/// # Runs a trillium handler in an async context with default config
///
/// Run the provided trillium handler on an already-running async-executor
/// with default settings. The defaults are the same as [`crate::run`]. To
/// customize these settings, see [`crate::config`].
///
/// This function will poll pending until the server shuts down.
pub async fn run_async(handler: impl Handler) {
    config().run_async(handler).await
}

/// # Configures a server before running it
///
/// ## Defaults
///
/// The default configuration is as follows:
///
/// port: the contents of the `PORT` env var or else 8080
/// host: the contents of the `HOST` env var or else "localhost"
/// signals handling and graceful shutdown: enabled on cfg(unix) systems
/// tcp nodelay: disabled
/// tls acceptor: none
///
/// ## Usage
///
/// ```rust
/// let swansong = trillium_smol::Swansong::new();
/// # swansong.shut_down(); // stoppping the server immediately for the test
/// trillium_smol::config()
///     .with_port(0)
///     .with_host("127.0.0.1")
///     .without_signals()
///     .with_nodelay()
///     .with_acceptor(()) // see [`trillium_rustls`], [`trillium_native_tls`], and [`trillium_openssl`]
///     .with_swansong(swansong)
///     .run(|conn: trillium::Conn| async move { conn.ok("hello smol") });
/// ```
///
/// See [`trillium_server_common::Config`] for more details
pub fn config() -> Config<()> {
    Config::new()
}
