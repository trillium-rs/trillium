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
# Trillium adapter using smol and async-global-executor

## Default / 12-factor applications

```rust,no_run
trillium_smol::run(|conn: trillium::Conn| async move {
    conn.ok("hello smol")
});
```

## Server configuration

For more details, see [trillium_smol::config](crate::config).

```rust
let swansong = trillium_smol::Swansong::new();
# swansong.shut_down(); // stoppping the server immediately for the test
trillium_smol::config()
    .with_port(0)
    .with_host("127.0.0.1")
    .without_signals()
    .with_nodelay()
    .with_acceptor(()) // see [`trillium_rustls`] and [`trillium_native_tls`]
    .with_swansong(swansong)
    .run(|conn: trillium::Conn| async move {
        conn.ok("hello smol")
    });
```

## Client

```rust
# #[cfg(feature = "smol")]
trillium_testing::with_server("ok", |url| async move {
    use trillium_smol::TcpConnector;
    use trillium_client::{Conn, Client};
    let mut conn = Conn::<TcpConnector>::get(url.clone()).execute().await?;
    assert_eq!(conn.response_body().read_string().await?, "ok");

    let client = Client::<TcpConnector>::new().with_default_pool();
    let mut conn = client.get(url);
    conn.send().await?;
    assert_eq!(conn.response_body().read_string().await?, "ok");
    Ok(())
});
```


*/

use trillium::Handler;
pub use trillium_server_common::{Binding, Swansong};

mod client;
pub use client::ClientConfig;

mod server;
use server::Config;

mod transport;
pub use transport::SmolTransport;

pub use async_global_executor;
pub use async_io;
pub use async_net;

/**
# Runs a trillium handler in a sync context with default config

Runs a trillium handler on the async-global-executor runtime with
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

Run the provided trillium handler on an already-running async-executor
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
let swansong = trillium_smol::Swansong::new();
# swansong.shut_down(); // stoppping the server immediately for the test
trillium_smol::config()
    .with_port(0)
    .with_host("127.0.0.1")
    .without_signals()
    .with_nodelay()
    .with_acceptor(()) // see [`trillium_rustls`] and [`trillium_native_tls`]
    .with_swansong(swansong)
    .run(|conn: trillium::Conn| async move {
        conn.ok("hello smol")
    });
```

See [`trillium_server_common::Config`] for more details

*/
pub fn config() -> Config<()> {
    Config::new()
}

/// spawn and detach a Future that returns ()
pub fn spawn<Fut: std::future::Future<Output = ()> + Send + 'static>(future: Fut) {
    async_global_executor::spawn(future).detach();
}
