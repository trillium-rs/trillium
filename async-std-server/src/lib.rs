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
# Trillium server adapter for async-std

```rust,no_run
fn main() {
    trillium_async_std_server::run(|conn: trillium::Conn| async move {
        conn.ok("hello async-std")
    });
}
```

```rust,no_run
#[async_std::main]
async fn main() {
    trillium_async_std_server::run_async(|conn: trillium::Conn| async move {
        conn.ok("hello async-std")
    }).await;
}
```
*/

use async_std::{
    net::{TcpListener, TcpStream},
    prelude::*,
    task,
};
use std::sync::Arc;
use trillium::{async_trait, Handler};
use trillium_server_common::{Acceptor, ConfigExt, Server};

pub use trillium_server_common::Stopper;

#[cfg(unix)]
async fn handle_signals(stop: Stopper) {
    use signal_hook::consts::signal::*;
    use signal_hook_async_std::Signals;

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
    pub struct AsyncStdServer;
    pub type Config<A> = trillium_server_common::Config<AsyncStdServer, A>;
}
use server::*;
#[async_trait]
impl Server for AsyncStdServer {
    type Transport = TcpStream;

    fn run<A: Acceptor<Self::Transport>, H: Handler>(config: Config<A>, handler: H) {
        task::block_on(async move { Self::run_async(config, handler).await })
    }

    async fn run_async<A: Acceptor<Self::Transport>, H: Handler>(
        config: Config<A>,
        mut handler: H,
    ) {
        if config.should_register_signals() {
            #[cfg(unix)]
            task::spawn(handle_signals(config.stopper()));
            #[cfg(not(unix))]
            panic!("signals handling not supported on windows yet");
        }

        let listener = config.build_listener::<TcpListener>();
        let mut incoming = config.stopper().stop_stream(listener.incoming());
        handler.init().await;
        let handler = Arc::new(handler);

        while let Some(Ok(stream)) = incoming.next().await {
            trillium::log_error!(stream.set_nodelay(config.nodelay()));
            task::spawn(config.clone().handle_stream(stream, handler.clone()));
        }

        config.graceful_shutdown().await;
    }
}

/**
# Runs a trillium handler in a sync context with default config

Runs a trillium handler on the async-std runtime with default
configuration. See [`crate::config`] for what the defaults are and how
to override them


This function will block the current thread until the server shuts
down
*/

pub fn run(handler: impl Handler) {
    config().run(handler)
}

/**
# Runs a trillium handler in an async context with default config

Run the provided trillium handler on an already-running async-std
runtime with default settings. the defaults are the same as
[`crate::run`]. To customize these settings, see [`crate::config`].

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
let stopper = trillium_async_std_server::Stopper::new();
# stopper.stop(); // stoppping the server immediately for the test
trillium_async_std_server::config()
    .with_port(8082)
    .with_host("0.0.0.0")
    .without_signals()
    .with_nodelay()
    .with_acceptor(()) // see [`trillium_rustls`] and [`trillium_native_tls`]
    .with_stopper(stopper)
    .run(|conn: trillium::Conn| async move {
        conn.ok("hello async-std")
    });
```

See [`trillium_server_common::Config`] for more details

*/
pub fn config() -> Config<()> {
    Config::new()
}
