use crate::{Acceptor, RuntimeTrait, Server, ServerHandle};
use async_cell::sync::AsyncCell;
use std::{
    cell::OnceCell,
    marker::PhantomData,
    net::SocketAddr,
    sync::{Arc, RwLock},
};
use trillium::{Handler, HttpConfig, Info, Swansong};

/**
# Primary entrypoint for configuring and running a trillium server

The associated methods on this struct are intended to be chained.

## Example
```rust,no_run
trillium_smol::config() // or trillium_async_std, trillium_tokio
    .with_port(8080) // the default
    .with_host("localhost") // the default
    .with_nodelay()
    .with_max_connections(Some(10000))
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
    * On `cfg(unix)` systems only: If the host string (as set by env var
      or direct config) begins with `.`, `/`, or `~`, it is
      interpreted to be a path, and trillium will bind to it as a unix
      domain socket. Port will be ignored. The socket will be deleted
      on clean shutdown.
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

#[derive(Debug)]
pub struct Config<ServerType: Server, AcceptorType> {
    pub(crate) acceptor: AcceptorType,
    pub(crate) port: Option<u16>,
    pub(crate) host: Option<String>,
    pub(crate) nodelay: bool,
    pub(crate) swansong: Swansong,
    pub(crate) register_signals: bool,
    pub(crate) max_connections: Option<usize>,
    pub(crate) info: Arc<AsyncCell<Arc<Info>>>,
    pub(crate) binding: RwLock<Option<ServerType>>,
    pub(crate) server: PhantomData<ServerType>,
    pub(crate) http_config: HttpConfig,
    pub(crate) runtime: ServerType::Runtime,
}

impl<ServerType, AcceptorType> Config<ServerType, AcceptorType>
where
    ServerType: Server,
    AcceptorType: Acceptor<ServerType::Transport>,
{
    /// Starts an async runtime and runs the provided handler with
    /// this config in that runtime. This is the appropriate
    /// entrypoint for applications that do not need to spawn tasks
    /// outside of trillium's web server. For applications that embed a
    /// trillium server inside of an already-running async runtime, use
    /// [`Config::run_async`]
    pub fn run<H: Handler>(self, h: H) {
        ServerType::run(self, h)
    }

    /// Runs the provided handler with this config, in an
    /// already-running runtime. This is the appropriate entrypoint
    /// for an application that needs to spawn async tasks that are
    /// unrelated to the trillium application. If you do not need to spawn
    /// other tasks, [`Config::run`] is the preferred entrypoint
    pub async fn run_async(self, handler: impl Handler) {
        let swansong = self.swansong.clone();
        ServerType::run_async(self, handler).await;
        swansong.shut_down().await;
    }

    /// Spawns the server onto the async runtime, returning a
    /// ServerHandle that can be awaited directly to return an
    /// [`Info`] or used with [`ServerHandle::info`] and
    /// [`ServerHandle::stop`]
    pub fn spawn(self, handler: impl Handler) -> ServerHandle {
        let server_handle = self.handle();
        self.runtime.clone().spawn(self.run_async(handler));
        server_handle
    }

    /// Returns a [`ServerHandle`] for this Config. This is useful
    /// when spawning the server onto a runtime.
    pub fn handle(&self) -> ServerHandle {
        ServerHandle {
            swansong: self.swansong.clone(),
            info: self.info.clone(),
            received_info: OnceCell::new(),
            runtime: self.runtime().into(),
        }
    }

    /// Configures the server to listen on this port. The default is
    /// the PORT environment variable or 8080
    pub fn with_port(mut self, port: u16) -> Self {
        if self.has_binding() {
            eprintln!("constructing a config with both a port and a pre-bound listener will ignore the port. this may be a panic in the future");
        }
        self.port = Some(port);
        self
    }

    /// Configures the server to listen on this host or ip
    /// address. The default is the HOST environment variable or
    /// "localhost"
    pub fn with_host(mut self, host: &str) -> Self {
        if self.has_binding() {
            eprintln!("constructing a config with both a host and a pre-bound listener will ignore the host. this may be a panic in the future");
        }
        self.host = Some(host.into());
        self
    }

    /// Configures the server to NOT register for graceful-shutdown
    /// signals with the operating system. Default behavior is for the
    /// server to listen for SIGINT and SIGTERM and perform a graceful
    /// shutdown.
    pub fn without_signals(mut self) -> Self {
        self.register_signals = false;
        self
    }

    /// Configures the tcp listener to use TCP_NODELAY. See
    /// <https://en.wikipedia.org/wiki/Nagle%27s_algorithm> for more
    /// information on this setting.
    pub fn with_nodelay(mut self) -> Self {
        self.nodelay = true;
        self
    }

    /// Configures the server to listen on the ip and port specified
    /// by the provided socketaddr. This is identical to
    /// `self.with_host(&socketaddr.ip().to_string()).with_port(socketaddr.port())`
    pub fn with_socketaddr(self, socketaddr: SocketAddr) -> Self {
        self.with_host(&socketaddr.ip().to_string())
            .with_port(socketaddr.port())
    }

    /// Configures the tls acceptor for this server
    pub fn with_acceptor<A: Acceptor<ServerType::Transport>>(
        self,
        acceptor: A,
    ) -> Config<ServerType, A> {
        Config {
            acceptor,
            host: self.host,
            port: self.port,
            nodelay: self.nodelay,
            server: PhantomData,
            swansong: self.swansong,
            register_signals: self.register_signals,
            max_connections: self.max_connections,
            info: self.info,
            binding: self.binding,
            http_config: self.http_config,
            runtime: self.runtime,
        }
    }

    /// use the specific [`Swansong`] provided
    pub fn with_swansong(mut self, swansong: Swansong) -> Self {
        self.swansong = swansong;
        self
    }

    /**
    Configures the maximum number of connections to accept. The
    default is 75% of the soft rlimit_nofile (`ulimit -n`) on unix
    systems, and None on other sytems.
    */
    pub fn with_max_connections(mut self, max_connections: Option<usize>) -> Self {
        self.max_connections = max_connections;
        self
    }

    /// configures trillium-http performance and security tuning parameters.
    ///
    /// See [`HttpConfig`] for documentation
    pub fn with_http_config(mut self, http_config: HttpConfig) -> Self {
        self.http_config = http_config;
        self
    }

    /// Use a pre-bound transport stream as server.
    ///
    /// The argument to this varies for different servers, but usually
    /// accepts the runtime's TcpListener and, on unix platforms, the UnixListener.
    ///
    /// ## Note well
    ///
    /// Many of the other options on this config will be ignored if you provide a listener. In
    /// particular, `host` and `port` will be ignored. All of the other options will be used.
    ///
    /// Additionally, cloning this config will not clone the listener.
    pub fn with_prebound_server(mut self, server: impl Into<ServerType>) -> Self {
        if self.host.is_some() {
            eprintln!("constructing a config with both a host and a pre-bound listener will ignore the host. this may be a panic in the future");
        }

        if self.port.is_some() {
            eprintln!("constructing a config with both a port and a pre-bound listener will ignore the port. this may be a panic in the future");
        }

        self.binding = RwLock::new(Some(server.into()));
        self
    }

    fn has_binding(&self) -> bool {
        self.binding
            .read()
            .as_deref()
            .map_or(false, Option::is_some)
    }

    /// retrieve the runtime
    pub fn runtime(&self) -> ServerType::Runtime {
        self.runtime.clone()
    }
}

impl<ServerType: Server> Config<ServerType, ()> {
    /// build a new config with default acceptor
    pub fn new() -> Self {
        Self::default()
    }
}

impl<ServerType: Server> Default for Config<ServerType, ()> {
    fn default() -> Self {
        #[cfg(unix)]
        let max_connections = {
            rlimit::getrlimit(rlimit::Resource::NOFILE)
                .ok()
                .and_then(|(soft, _hard)| soft.try_into().ok())
                .map(|limit: usize| 3 * limit / 4)
        };

        #[cfg(not(unix))]
        let max_connections = None;

        log::debug!("using max connections of {:?}", max_connections);

        Self {
            acceptor: (),
            port: None,
            host: None,
            server: PhantomData,
            nodelay: false,
            swansong: Swansong::new(),
            register_signals: cfg!(unix),
            max_connections,
            info: AsyncCell::shared(),
            binding: RwLock::new(None),
            http_config: HttpConfig::default(),
            runtime: ServerType::runtime(),
        }
    }
}
