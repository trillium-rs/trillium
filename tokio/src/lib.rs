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
//! # Trillium server adapter for tokio
//!
//! ```rust,no_run
//! # #[allow(clippy::needless_doctest_main)]
//! fn main() {
//!     trillium_tokio::run(|conn: trillium::Conn| async move { conn.ok("hello tokio") });
//! }
//! ```
//!
//! ```rust,no_run
//! # #[allow(clippy::needless_doctest_main)]
//! #[tokio::main]
//! async fn main() {
//!     trillium_tokio::run_async(|conn: trillium::Conn| async move { conn.ok("hello tokio") })
//!         .await;
//! }
//! ```
//!
//! For advanced binding — several listeners on one server, fallible binds you
//! can recover from, or adopting an already-bound socket — call
//! [`.listeners()`](trillium_server_common::Config::listeners)
//! on a [`config()`](trillium_server_common::Config) to get a [`ListenerConfig`].
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
//!     trillium_tokio::config()
//!         .listeners()
//!         .bind_reuseport_tcp(8080)?
//!         .run(|conn: Conn| async move { conn.ok("hello") });
//!     Ok(())
//! }
//! ```
//!
//! Each worker thread runs its own single-threaded runtime, pinned to a core,
//! driving that worker's accept loop — one `SO_REUSEPORT` listener per worker.
//! The standard multi-threaded work-stealing runtime is still present alongside
//! them, hosting HTTP/3, signal handling, and the application tasks you spawn,
//! so QUIC is never fanned out this way. Set the worker count with
//! `.with_reuseport_workers(n)`; it defaults to the `WORKERS` environment
//! variable, or if that's not set, to available parallelism.
//!
//! This trades the work-stealing runtime's load balancing for per-core
//! locality, which can improve throughput for short, CPU-cheap requests served
//! over many connections. It is gated off on platforms where plain
//! `SO_REUSEPORT` does not distribute connections (including macOS), where it
//! would offer no benefit.

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

use trillium::Handler;
pub use trillium_server_common::{Binding, IntoListenAddr, ListenerConfig, Swansong};

mod client;
pub use client::ClientConfig;
#[cfg(unix)]
pub use client::UnixClientConfig;

mod server;
pub use async_compat;
use server::Config;
pub use tokio;
pub use tokio_stream;

mod transport;
pub use transport::TokioTransport;

/// # Runs a trillium handler in a sync context with default config
///
/// Runs a trillium handler on the tokio runtime with
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
/// Run the provided trillium handler on an already-running tokio runtime
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
/// let swansong = trillium_tokio::Swansong::new();
/// # swansong.shut_down(); // stoppping the server immediately for the test
/// trillium_tokio::config()
///     .with_port(0)
///     .with_host("127.0.0.1")
///     .without_signals()
///     .with_nodelay()
///     .with_acceptor(()) // see [`trillium_rustls`], [`trillium_native_tls`], and [`trillium_openssl`]
///     .with_swansong(swansong)
///     .run(|conn: trillium::Conn| async move { conn.ok("hello tokio") });
/// ```
///
/// See [`trillium_server_common::Config`] for more details
pub fn config() -> Config<()> {
    Config::new()
}

mod runtime;
pub use runtime::TokioRuntime;

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
pub use udp::TokioUdpSocket;
