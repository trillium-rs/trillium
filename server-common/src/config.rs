use crate::{
    Acceptor, ArcHandler, QuicConfig, RuntimeTrait, Server, ServerHandle,
    running_config::RunningConfig,
};
use async_cell::sync::AsyncCell;
use futures_lite::StreamExt;
use std::{cell::OnceCell, net::SocketAddr, pin::pin, sync::Arc};
use trillium::{Handler, Headers, HttpConfig, Info, KnownHeaderName, SERVER, Swansong, TypeSet};
use trillium_http::HttpContext;
use url::Url;

/// # Primary entrypoint for configuring and running a trillium server
///
/// The associated methods on this struct are intended to be chained.
///
/// ## Example
/// ```rust,no_run
/// trillium_smol::config() // or trillium_async_std, trillium_tokio
///     .with_port(8080) // the default
///     .with_host("localhost") // the default
///     .with_nodelay()
///     .with_max_connections(Some(10000))
///     .without_signals()
///     .run(|conn: trillium::Conn| async move { conn.ok("hello") });
/// ```
///
/// # Socket binding
///
/// The socket binding logic is as follows:
///
/// If a LISTEN_FD environment variable is available on `cfg(unix)`
/// systems, that will be used, overriding host and port settings
/// Otherwise:
/// Host will be selected from explicit configuration using
/// [`Config::with_host`] or else the `HOST` environment variable,
/// or else a default of "localhost".
/// On `cfg(unix)` systems only: If the host string (as set by env var
/// or direct config) begins with `.`, `/`, or `~`, it is
/// interpreted to be a path, and trillium will bind to it as a unix
/// domain socket. Port will be ignored. The socket will be deleted
/// on clean shutdown.
/// Port will be selected from explicit configuration using
/// [`Config::with_port`] or else the `PORT` environment variable,
/// or else a default of 8080.
///
/// ## Signals
///
/// On `cfg(unix)` systems, `SIGTERM`, `SIGINT`, and `SIGQUIT` are all
/// registered to perform a graceful shutdown on the first signal and an
/// immediate shutdown on a subsequent signal. This behavior may change as
/// trillium matures. To disable this behavior, use
/// [`Config::without_signals`].
#[derive(Debug)]
pub struct Config<ServerType: Server, AcceptorType, QuicType: QuicConfig<ServerType> = ()> {
    pub(crate) acceptor: AcceptorType,
    pub(crate) quic: QuicType,
    pub(crate) binding: Option<ServerType>,
    pub(crate) host: Option<String>,
    pub(crate) context_cell: Arc<AsyncCell<Arc<HttpContext>>>,
    pub(crate) max_connections: Option<usize>,
    pub(crate) nodelay: bool,
    pub(crate) port: Option<u16>,
    pub(crate) register_signals: bool,
    pub(crate) runtime: ServerType::Runtime,
    pub(crate) context: HttpContext,
}

impl<ServerType, AcceptorType, QuicType> Config<ServerType, AcceptorType, QuicType>
where
    ServerType: Server,
    AcceptorType: Acceptor<ServerType::Transport>,
    QuicType: QuicConfig<ServerType>,
{
    /// Starts an async runtime and runs the provided handler with
    /// this config in that runtime. This is the appropriate
    /// entrypoint for applications that do not need to spawn tasks
    /// outside of trillium's web server. For applications that embed a
    /// trillium server inside of an already-running async runtime, use
    /// [`Config::run_async`]
    pub fn run(self, handler: impl Handler) {
        self.runtime.clone().block_on(self.run_async(handler));
    }

    /// Runs the provided handler with this config, in an
    /// already-running runtime. This is the appropriate entrypoint
    /// for an application that needs to spawn async tasks that are
    /// unrelated to the trillium application. If you do not need to spawn
    /// other tasks, [`Config::run`] is the preferred entrypoint
    pub async fn run_async(self, mut handler: impl Handler) {
        let Self {
            runtime,
            acceptor,
            quic,
            mut max_connections,
            nodelay,
            binding,
            host,
            port,
            register_signals,
            context,
            context_cell,
        } = self;

        #[cfg(unix)]
        if max_connections.is_none() {
            max_connections = rlimit::getrlimit(rlimit::Resource::NOFILE)
                .ok()
                .and_then(|(soft, _hard)| soft.try_into().ok())
                .map(|limit: usize| ((limit as f32) * 0.75) as usize);
        };

        log::debug!("using max connections of {:?}", max_connections);

        let host = host
            .or_else(|| std::env::var("HOST").ok())
            .unwrap_or_else(|| "localhost".into());
        let port = port
            .or_else(|| {
                std::env::var("PORT")
                    .ok()
                    .map(|x| x.parse().expect("PORT must be an unsigned integer"))
            })
            .unwrap_or(8080);

        let listener = binding
            .inspect(|_| log::debug!("taking prebound listener"))
            .unwrap_or_else(|| ServerType::from_host_and_port(&host, port));

        let swansong = context.swansong().clone();

        let mut info = Info::from(context)
            .with_state(runtime.clone().into())
            .with_state(runtime.clone());

        info.state_entry::<Headers>()
            .or_default()
            .try_insert(KnownHeaderName::Server, SERVER);

        listener.init(&mut info);

        let quic_binding = if let Some(socket_addr) = info.tcp_socket_addr().copied() {
            let quic_binding = quic
                .bind(socket_addr, runtime.clone(), &mut info)
                .map(|r| r.expect("failed to bind QUIC endpoint"));

            if quic_binding.is_some() {
                info.state_entry::<Headers>()
                    .or_default()
                    .try_insert_with(KnownHeaderName::AltSvc, || {
                        format!("h3=\":{}\"", socket_addr.port())
                    });
            }

            quic_binding
        } else {
            None
        };

        insert_url(info.as_mut(), acceptor.is_secure());

        handler.init(&mut info).await;

        let context = Arc::new(HttpContext::from(info));

        context_cell.set(context.clone());

        if register_signals {
            let runtime = runtime.clone();
            runtime.clone().spawn(async move {
                let mut signals = pin!(runtime.hook_signals([2, 3, 15]));
                while signals.next().await.is_some() {
                    let guard_count = swansong.guard_count();
                    if swansong.state().is_shutting_down() {
                        eprintln!(
                            "\nSecond interrupt, shutting down harshly (dropping {guard_count} \
                             guards)"
                        );
                        std::process::exit(1);
                    } else {
                        println!(
                            "\nShutting down gracefully. Waiting for {guard_count} shutdown \
                             guards to drop.\nControl-c again to force."
                        );
                        swansong.shut_down();
                    }
                }
            });
        }

        let handler = ArcHandler::new(handler);

        if let Some(quic_binding) = quic_binding {
            let context = context.clone();
            let handler = handler.clone();
            let runtime: crate::Runtime = runtime.clone().into();
            runtime.clone().spawn(crate::h3::run_h3(
                quic_binding,
                context,
                handler,
                runtime,
            ));
        }

        let running_config = Arc::new(RunningConfig {
            acceptor,
            max_connections,
            context,
            runtime,
            nodelay,
        });

        running_config.run_async(listener, handler).await;
    }

    /// Spawns the server onto the async runtime, returning a
    /// ServerHandle that can be awaited directly to return an
    /// [`Info`] or used with [`ServerHandle::info`] and
    /// [`ServerHandle::shut_down`]
    pub fn spawn(self, handler: impl Handler) -> ServerHandle {
        let server_handle = self.handle();
        self.runtime.clone().spawn(self.run_async(handler));
        server_handle
    }

    /// Returns a [`ServerHandle`] for this Config. This is useful
    /// when spawning the server onto a runtime.
    pub fn handle(&self) -> ServerHandle {
        ServerHandle {
            swansong: self.context.swansong().clone(),
            context: self.context_cell.clone(),
            received_context: OnceCell::new(),
            runtime: self.runtime().into(),
        }
    }

    /// Configures the server to listen on this port. The default is
    /// the PORT environment variable or 8080
    pub fn with_port(mut self, port: u16) -> Self {
        if self.has_binding() {
            eprintln!(
                "constructing a config with both a port and a pre-bound listener will ignore the \
                 port. this may be a panic in the future"
            );
        }
        self.port = Some(port);
        self
    }

    /// Configures the server to listen on this host or ip
    /// address. The default is the HOST environment variable or
    /// "localhost"
    pub fn with_host(mut self, host: &str) -> Self {
        if self.has_binding() {
            eprintln!(
                "constructing a config with both a host and a pre-bound listener will ignore the \
                 host. this may be a panic in the future"
            );
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
    ) -> Config<ServerType, A, QuicType> {
        Config {
            acceptor,
            quic: self.quic,
            host: self.host,
            port: self.port,
            nodelay: self.nodelay,
            register_signals: self.register_signals,
            max_connections: self.max_connections,
            context_cell: self.context_cell,
            context: self.context,
            binding: self.binding,
            runtime: self.runtime,
        }
    }

    /// Configures QUIC/HTTP3 for this server
    pub fn with_quic<Q: QuicConfig<ServerType>>(
        self,
        quic: Q,
    ) -> Config<ServerType, AcceptorType, Q> {
        Config {
            acceptor: self.acceptor,
            quic,
            host: self.host,
            port: self.port,
            nodelay: self.nodelay,
            register_signals: self.register_signals,
            max_connections: self.max_connections,
            context_cell: self.context_cell,
            context: self.context,
            binding: self.binding,
            runtime: self.runtime,
        }
    }

    /// use the specific [`Swansong`] provided
    pub fn with_swansong(mut self, swansong: Swansong) -> Self {
        self.context.set_swansong(swansong);
        self
    }

    /// Configures the maximum number of connections to accept. The
    /// default is 75% of the soft rlimit_nofile (`ulimit -n`) on unix
    /// systems, and None on other sytems.
    pub fn with_max_connections(mut self, max_connections: Option<usize>) -> Self {
        self.max_connections = max_connections;
        self
    }

    /// configures trillium-http performance and security tuning parameters.
    ///
    /// See [`HttpConfig`] for documentation
    pub fn with_http_config(mut self, http_config: HttpConfig) -> Self {
        *self.context.http_config_mut() = http_config;
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
            eprintln!(
                "constructing a config with both a host and a pre-bound listener will ignore the \
                 host. this may be a panic in the future"
            );
        }

        if self.port.is_some() {
            eprintln!(
                "constructing a config with both a port and a pre-bound listener will ignore the \
                 port. this may be a panic in the future"
            );
        }

        self.binding = Some(server.into());
        self
    }

    fn has_binding(&self) -> bool {
        self.binding.is_some()
    }

    /// retrieve the runtime
    pub fn runtime(&self) -> ServerType::Runtime {
        self.runtime.clone()
    }

    /// return the configured port
    pub fn port(&self) -> Option<u16> {
        self.port
    }

    /// return the configured host
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
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
        Self {
            acceptor: (),
            quic: (),
            port: None,
            host: None,
            nodelay: false,
            register_signals: cfg!(unix),
            max_connections: None,
            context_cell: AsyncCell::shared(),
            binding: None,
            runtime: ServerType::runtime(),
            context: Default::default(),
        }
    }
}

fn insert_url(state: &mut TypeSet, secure: bool) -> Option<()> {
    let socket_addr = state.get::<SocketAddr>().copied()?;
    let vacant_entry = state.entry::<Url>().into_vacant()?;

    let host = if socket_addr.ip().is_loopback() {
        "localhost".to_string()
    } else {
        socket_addr.ip().to_string()
    };

    let url = match (secure, socket_addr.port()) {
        (true, 443) => format!("https://{host}"),
        (false, 80) => format!("http://{host}"),
        (true, port) => format!("https://{host}:{port}/"),
        (false, port) => format!("http://{host}:{port}/"),
    };

    let url = Url::parse(&url).ok()?;

    vacant_entry.insert(url);
    Some(())
}
