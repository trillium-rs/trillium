use crate::CloneCounter;

use trillium_http::Stopper;

/**
# Primary entrypoint for configuring and running a trillium server

The associated methods on this struct are intended to be chained.

## Example
```rust,no_run
trillium_smol::config() // or trillium_async_std, trillium_tokio
    .with_port(8080) // the default
    .with_host("localhost") // the default
    .with_nodelay()
    .without_signals()
    .run(|conn: trillium::Conn| async move { conn.ok("hello") });
```

# Socket binding

The socket binding logic is as follows:

* If a LISTEN_FD environment variable is available on `cfg(unix)`
  systems, that will be used, overriding host and port settings
* Otherwise:
  * Host will be selected from explicit configuration using
    [`Config::with_host`] or else the `HOST` environment variable,
    or else a default of "localhost".
  * Port will be selected from explicit configuration using
    [`Config::with_port`] or else the `PORT` environment variable,
    or else a default of 8080.

## Signals

On `cfg(unix)` systems, `SIGTERM`, `SIGINT`, and `SIGQUIT` are all
registered to perform a graceful shutdown on the first signal and an
immediate shutdown on a subsequent signal. This behavior may change as
trillium matures. To disable this behavior, use
[`Config::without_signals`].

## For runtime adapter authors

In order to use this to _implement_ a trillium server, see
[`trillium_server_common::ConfigExt`](crate::ConfigExt)
*/

#[derive(Debug, Clone)]
pub struct Config {
    pub(crate) port: Option<u16>,
    pub(crate) host: Option<String>,
    pub(crate) nodelay: bool,
    pub(crate) stopper: Stopper,
    pub(crate) counter: CloneCounter,
    pub(crate) register_signals: bool,
}

impl Config {
    /// build a new config
    pub fn new() -> Self {
        Self::default()
    }

    /// Configures the server to listen on this port. The default is
    /// the PORT environment variable or 8080
    pub fn with_port(mut self, port: u16) -> Self {
        self.set_port(port);
        self
    }

    /// Configures the server to listen on this host or ip
    /// address. The default is the HOST environment variable or
    /// "localhost"
    pub fn with_host(mut self, host: &str) -> Self {
        self.set_host(host);
        self
    }

    /// Configures the server to NOT register for graceful-shutdown
    /// signals with the operating system. Default behavior is for the
    /// server to listen for SIGINT and SIGTERM and perform a graceful
    /// shutdown.
    pub fn without_signals(mut self) -> Self {
        self.set_register_signals(false);
        self
    }

    /// Configures the tcp listener to use TCP_NODELAY. See
    /// <https://en.wikipedia.org/wiki/Nagle%27s_algorithm> for more
    /// information on this setting.
    pub fn with_nodelay(mut self) -> Self {
        self.set_nodelay(true);
        self
    }

    /// use the specific [`Stopper`] provided
    pub fn with_stopper(mut self, stopper: Stopper) -> Self {
        self.set_stopper(stopper);
        self
    }

    /// replace the stopper
    pub fn set_stopper(&mut self, stopper: Stopper) {
        self.stopper = stopper;
    }

    /// get the [`Stopper`]
    pub fn stopper(&self) -> &Stopper {
        &self.stopper
    }

    /// set the host
    pub fn set_host(&mut self, host: &str) {
        self.host = Some(host.into());
    }

    /// get the host as specified
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// get the port as specified
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// set the port
    pub fn set_port(&mut self, port: u16) {
        self.port = Some(port);
    }

    /// get the nodelay setting
    pub fn nodelay(&self) -> bool {
        self.nodelay
    }

    /// get the [`CloneCounter`]
    pub fn counter(&self) -> &CloneCounter {
        &self.counter
    }

    /// get whether signals were requested
    pub fn register_signals(&self) -> bool {
        self.register_signals
    }

    /// set whether we should register signals
    pub fn set_register_signals(&mut self, register_signals: bool) {
        self.register_signals = register_signals;
    }

    /// sets whether tcp_nodelay is enabled
    pub fn set_nodelay(&mut self, nodelay: bool) {
        self.nodelay = nodelay;
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: None,
            host: None,
            nodelay: false,
            stopper: Stopper::new(),
            counter: CloneCounter::new(),
            register_signals: cfg!(unix),
        }
    }
}
