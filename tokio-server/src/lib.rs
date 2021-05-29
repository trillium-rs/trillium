#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]
/*!
# Trillium server adapter for tokio

```rust,no_run
fn main() {
    trillium_tokio_server::run(|conn: trillium::Conn| async move {
        conn.ok("hello tokio")
    });
}
```

```rust,no_run
#[tokio::main]
async fn main() {
    trillium_tokio_server::run_async(|conn: trillium::Conn| async move {
        conn.ok("hello tokio")
    }).await;
}
```
*/

use async_compat::Compat;
use futures::stream::StreamExt;
use std::sync::Arc;
use tokio::{
    net::{TcpListener, TcpStream},
    runtime::Runtime,
};
use tokio_stream::wrappers::TcpListenerStream;
use trillium::{async_trait, Handler};
use trillium_server_common::{Acceptor, ConfigExt, Server};

pub use trillium_server_common::Stopper;

#[cfg(unix)]
async fn handle_signals(stop: Stopper) {
    use signal_hook::consts::signal::*;
    use signal_hook_tokio::Signals;
    let signals = Signals::new(&[SIGINT, SIGTERM, SIGQUIT]).unwrap();
    let mut signals = signals.fuse();

    while signals.next().await.is_some() {
        if stop.is_stopped() {
            println!("second interrupt, shutting down harshly");
            std::process::exit(1);
        } else {
            println!("shutting down gracefully");
            stop.stop();
        }
    }
}
mod server {
    #[derive(Debug, Clone, Copy)]
    pub struct TokioServer;
    pub type Config<A> = trillium_server_common::Config<TokioServer, A>;
}
use server::*;

#[async_trait]
impl Server for TokioServer {
    type Transport = Compat<TcpStream>;

    fn run<A: Acceptor<Self::Transport>, H: Handler>(config: Config<A>, handler: H) {
        Runtime::new()
            .unwrap()
            .block_on(async move { Self::run_async(config, handler).await });
    }

    async fn run_async<A: Acceptor<Self::Transport>, H: Handler>(
        config: Config<A>,
        mut handler: H,
    ) {
        if config.should_register_signals() {
            #[cfg(unix)]
            tokio::spawn(handle_signals(config.stopper()));
            #[cfg(not(unix))]
            panic!("signals handling not supported on windows yet");
        }

        let listener = config.build_listener::<TcpListener>();
        handler.init().await;
        let handler = Arc::new(handler);

        let mut stream = config
            .stopper()
            .stop_stream(TcpListenerStream::new(listener));

        while let Some(Ok(stream)) = stream.next().await {
            trillium::log_error!(stream.set_nodelay(config.nodelay()));
            tokio::spawn(
                config
                    .clone()
                    .handle_stream(Compat::new(stream), handler.clone()),
            );
        }

        config.graceful_shutdown().await;
    }
}

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
let stopper = trillium_tokio_server::Stopper::new();
# stopper.stop(); // stoppping the server immediately for the test
trillium_tokio_server::config()
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
