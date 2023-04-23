use crate::{
    server_handle::CompletionFuture, Acceptor, CloneCounterObserver, Server, ServerHandle, Stopper,
};
use std::{marker::PhantomData, net::SocketAddr, sync::Arc};
use trillium::{Handler, Info};

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
pub struct Config<ServerType, AcceptorType> {
    pub(crate) acceptor: AcceptorType,
    pub(crate) port: Option<u16>,
    pub(crate) host: Option<String>,
    pub(crate) nodelay: bool,
    pub(crate) stopper: Stopper,
    pub(crate) observer: CloneCounterObserver,
    pub(crate) register_signals: bool,
    pub(crate) max_connections: Option<usize>,
    pub(crate) info: Arc<async_cell::sync::AsyncCell<Info>>,
    pub(crate) completion_future: CompletionFuture,
    server: PhantomData<ServerType>,
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
        let completion_future = self.completion_future.clone();
        ServerType::run_async(self, handler).await;
        completion_future.notify()
    }

    /// Spawns the server onto the async runtime, returning a
    /// ServerHandle that can be awaited directly to return an
    /// [`Info`] or used with [`ServerHandle::info`] and
    /// [`ServerHandle::stop`]
    pub fn spawn(self, handler: impl Handler) -> ServerHandle {
        let server_handle = self.handle();
        ServerType::spawn(self.run_async(handler));
        server_handle
    }

    /// Returns a [`ServerHandle`] for this Config. This is useful
    /// when spawning the server onto a runtime.
    pub fn handle(&self) -> ServerHandle {
        ServerHandle {
            stopper: self.stopper.clone(),
            info: self.info.clone(),
            completion: self.completion_future.clone(),
        }
    }

    /// Configures the server to listen on this port. The default is
    /// the PORT environment variable or 8080
    pub fn with_port(mut self, port: u16) -> Self {
        self.port = Some(port);
        self
    }

    /// Configures the server to listen on this host or ip
    /// address. The default is the HOST environment variable or
    /// "localhost"
    pub fn with_host(mut self, host: &str) -> Self {
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
            stopper: self.stopper,
            observer: self.observer,
            register_signals: self.register_signals,
            max_connections: self.max_connections,
            info: self.info,
            completion_future: self.completion_future,
        }
    }

    /// use the specific [`Stopper`] provided
    pub fn with_stopper(mut self, stopper: Stopper) -> Self {
        self.stopper = stopper;
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
}

impl<ServerType> Config<ServerType, ()> {
    /// build a new config with default acceptor
    pub fn new() -> Self {
        Self::default()
    }
}

impl<ServerType, AcceptorType: Clone> Clone for Config<ServerType, AcceptorType> {
    fn clone(&self) -> Self {
        Self {
            acceptor: self.acceptor.clone(),
            port: self.port,
            host: self.host.clone(),
            server: PhantomData,
            nodelay: self.nodelay,
            stopper: self.stopper.clone(),
            observer: self.observer.clone(),
            register_signals: self.register_signals,
            max_connections: self.max_connections,
            info: async_cell::sync::AsyncCell::shared(),
            completion_future: CompletionFuture::new(),
        }
    }
}

impl<ServerType> Default for Config<ServerType, ()> {
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
            stopper: Stopper::new(),
            observer: CloneCounterObserver::new(),
            register_signals: cfg!(unix),
            max_connections,
            info: async_cell::sync::AsyncCell::shared(),
            completion_future: CompletionFuture::new(),
        }
    }
}
