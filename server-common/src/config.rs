use crate::{CloneCounter, Server};
use std::marker::PhantomData;
use trillium::Handler;
use trillium_http::Stopper;
use trillium_tls_common::Acceptor;

/// # Primary entrypoint for configuring and running a trillium server
///
/// The associated methods on this struct are intended to be chained.
///
/// ## Example
/// ```rust
/// // in reality, you'd use trillium_smol, trillium_async_std, trillium_tokio, etc
/// trillium_testing::server::config()
///     .with_port(8080) // the default
///     .with_host("localhost") // the default
///     .with_nodelay()
///     .without_signals()
///     .run(|conn: trillium::Conn| async move { conn.ok("hello") });
/// ```
/// In order to use this to _implement_ a trillium server, see
/// [`trillium_server_common::ConfigExt`](crate::ConfigExt)

#[derive(Debug)]
pub struct Config<ServerType, AcceptorType> {
    pub(crate) acceptor: AcceptorType,
    pub(crate) port: Option<u16>,
    pub(crate) host: Option<String>,
    pub(crate) nodelay: bool,
    pub(crate) stopper: Stopper,
    pub(crate) counter: CloneCounter,
    pub(crate) register_signals: bool,
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
        ServerType::run_async(self, handler).await
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
            counter: self.counter,
            register_signals: self.register_signals,
        }
    }

    /// use the specific [`Stopper`] provided
    pub fn with_stopper(mut self, stopper: Stopper) -> Self {
        self.stopper = stopper;
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
            counter: self.counter.clone(),
            register_signals: self.register_signals,
        }
    }
}

impl<ServerType> Default for Config<ServerType, ()> {
    fn default() -> Self {
        Self {
            acceptor: (),
            port: None,
            host: None,
            server: PhantomData,
            nodelay: false,
            stopper: Stopper::new(),
            counter: CloneCounter::new(),
            register_signals: cfg!(unix),
        }
    }
}
