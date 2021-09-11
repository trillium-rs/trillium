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
# Trillium server adapter for tokio

```rust,no_run
# #[allow(clippy::needless_doctest_main)]
fn main() {
    trillium_tokio::run(|conn: trillium::Conn| async move {
        conn.ok("hello tokio")
    });
}
```

```rust,no_run
# #[allow(clippy::needless_doctest_main)]
#[tokio::main]
async fn main() {
    trillium_tokio::run_async(|conn: trillium::Conn| async move {
        conn.ok("hello tokio")
    }).await;
}
```
*/

use std::future::Future;

use trillium::Handler;
pub use trillium_server_common::Stopper;

mod client;
pub use client::{ClientConfig, TcpConnector};

mod server;
use server::Config;

pub use async_compat;
pub use tokio;
pub use tokio_stream;

/**
# Runs a trillium handler in a sync context with default config

Runs a trillium handler on the tokio runtime with
default configuration. See [`crate::config`] for what the defaults are
and how to override them


This function will block the current thread until the server shuts
down
*/
pub fn run(handler: impl Handler) {
    config().run(handler)
}

/**
# Runs a trillium handler in an async context with default config

Run the provided trillium handler on an already-running tokio runtime
with default settings. The defaults are the same as [`crate::run`]. To
customize these settings, see [`crate::config`].

This function will poll pending until the server shuts down.

*/
pub async fn run_async(handler: impl Handler) {
    config().run_async(handler).await
}

/**
# Configures a server before running it

## Defaults

The default configuration is as follows:

* port: the contents of the `PORT` env var or else 8080
* host: the contents of the `HOST` env var or else "localhost"
* signals handling and graceful shutdown: enabled on cfg(unix) systems
* tcp nodelay: disabled
* tls acceptor: none

## Usage

```rust
let stopper = trillium_tokio::Stopper::new();
# stopper.stop(); // stoppping the server immediately for the test
trillium_tokio::config()
    .with_port(8082)
    .with_host("0.0.0.0")
    .without_signals()
    .with_nodelay()
    .with_acceptor(()) // see [`trillium_rustls`] and [`trillium_native_tls`]
    .with_stopper(stopper)
    .run(|conn: trillium::Conn| async move {
        conn.ok("hello tokio")
    });
```

See [`trillium_server_common::Config`] for more details

*/
pub fn config() -> Config<()> {
    Config::new()
}

/**
reexport tokio runtime block_on
*/
pub fn block_on<Fut: Future<Output = T>, T>(future: Fut) -> T {
    tokio::runtime::Runtime::new().unwrap().block_on(future)
}
