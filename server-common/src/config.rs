use crate::{
    Acceptor, ArcHandler, BoxedAcceptor, BoxedQuicConfig, ListenerConfig, QuicConfig, RuntimeTrait,
    Server, ServerHandle,
    server::{PreboundListener, resolve_listener},
};
use async_cell::sync::AsyncCell;
use futures_lite::StreamExt;
use std::{
    cell::OnceCell,
    net::{SocketAddr, UdpSocket as StdUdpSocket},
    pin::pin,
    sync::Arc,
};
use trillium::{
    Handler, Headers, HttpConfig, Info, KnownHeaderName, Listener, Listeners, Swansong, TypeSet,
};
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
    pub async fn run_async(self, handler: impl Handler) {
        let Self {
            runtime,
            acceptor,
            quic,
            max_connections,
            nodelay,
            binding,
            host,
            port,
            register_signals,
            context,
            context_cell,
        } = self;

        // Materialize this single-listener configuration into a multi-listener `ListenerConfig`,
        // which owns the one accept-loop / signal / QUIC driver. `Config` is the opinionated front
        // door: it always resolves to exactly one binding (the 12-factor default if none was set),
        // preserving its panic-on-bind-failure contract at this seam, then delegates.
        let builder = ListenerConfig::<ServerType>::from_global(
            context,
            context_cell,
            runtime,
            max_connections,
            nodelay,
            register_signals,
        );

        let builder = match binding {
            // A prebound server is already adopted into the runtime; adopt it as-is. QUIC is not
            // attached here: a prebound server's address is opaque until `init`, and a prebound TCP
            // listener paired with a QUIC/UDP endpoint is not a meaningful combination. Bind QUIC
            // on a multi-listener `ListenerConfig` if both are genuinely needed.
            Some(server) => {
                if quic.is_configured() {
                    log::warn!(
                        "QUIC configuration is ignored when a prebound server is supplied; use a \
                         multi-listener ListenerConfig to bind QUIC explicitly"
                    );
                }
                log::debug!("taking prebound listener");
                builder.bind_server_boxed(server, BoxedAcceptor::new(acceptor))
            }

            None => {
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

                if quic.is_configured() {
                    // QUIC binds on the single listener's resolved address, so the TCP port must be
                    // known here to claim the matching UDP socket. Any QUIC-capable server uses the
                    // default `from_host_and_port`, so resolving via the shared `resolve_listener`
                    // is behaviorally identical while exposing the port. Bind failure panics, as
                    // the `Config` contract requires.
                    match resolve_listener(&host, port)
                        .unwrap_or_else(|e| panic!("failed to bind {host}:{port}: {e}"))
                    {
                        PreboundListener::Tcp(tcp) => {
                            let addr = tcp
                                .local_addr()
                                .expect("a bound tcp listener has a local address");
                            let builder = builder.push_listener(
                                PreboundListener::Tcp(tcp),
                                BoxedAcceptor::new(acceptor),
                            );
                            let socket = StdUdpSocket::bind(addr).unwrap_or_else(|e| {
                                panic!("failed to bind QUIC UDP socket at {addr}: {e}")
                            });
                            builder.push_quic_listener(socket, BoxedQuicConfig::new(quic))
                        }
                        #[cfg(unix)]
                        PreboundListener::Unix(unix) => {
                            log::warn!("QUIC configuration is ignored on a unix-domain listener");
                            builder.push_listener(
                                PreboundListener::Unix(unix),
                                BoxedAcceptor::new(acceptor),
                            )
                        }
                    }
                } else {
                    let server = ServerType::from_host_and_port(&host, port);
                    builder.bind_server_boxed(server, BoxedAcceptor::new(acceptor))
                }
            }
        };

        builder.run_async(handler).await;
    }

    /// Spawns the server onto the async runtime, returning a [`ServerHandle`].
    ///
    /// - `await server_handle` — waits for the server to shut down (output: `()`)
    /// - `server_handle.info().await` — waits for the server to finish binding, then returns
    ///   [`BoundInfo`](crate::server_handle::BoundInfo)
    /// - `server_handle.shut_down()` — initiates graceful shutdown
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
            log::warn!(
                "constructing a config with both a port and a pre-bound listener will ignore the \
                 port"
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
            log::warn!(
                "constructing a config with both a host and a pre-bound listener will ignore the \
                 host"
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
    /// systems, and None on other systems.
    pub fn with_max_connections(mut self, max_connections: Option<usize>) -> Self {
        self.max_connections = max_connections;
        self
    }

    /// configures trillium-http performance and security tuning parameters.
    ///
    /// See [`HttpConfig`] for documentation
    pub fn with_http_config(mut self, config: HttpConfig) -> Self {
        *self.context.config_mut() = config;
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
            log::warn!(
                "constructing a config with both a host and a pre-bound listener will ignore the \
                 host"
            );
        }

        if self.port.is_some() {
            log::warn!(
                "constructing a config with both a port and a pre-bound listener will ignore the \
                 port"
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

    /// add arbitrary state to the [`HttpContext`]'s [`TypeSet`](trillium::TypeSet) that will be
    /// available in the following places:
    ///
    /// - mutably on [`Info`](trillium::Info) as
    ///   [`Info::shared_state`](trillium::Info::shared_state) within
    ///   [`Handler::init`](trillium::Handler::init)
    /// - immutably on every [`Conn`](trillium::Conn) as
    ///   [`Conn::shared_state`](trillium::Conn::shared_state)
    /// - immutably on the [`BoundInfo`](crate::BoundInfo) as
    ///   [`BoundInfo::shared_state`](crate::BoundInfo::shared_state) returned by
    ///   [`ServerHandle::info`](crate::ServerHandle::info)
    pub fn with_shared_state<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.context.shared_state_mut().insert(state);
        self
    }

    /// add arbitrary state to the [`HttpContext`]'s [`TypeSet`](trillium::TypeSet) that will be
    /// available in the following places:
    ///
    /// - mutably on [`Info`](trillium::Info) as
    ///   [`Info::shared_state`](trillium::Info::shared_state) within
    ///   [`Handler::init`](trillium::Handler::init)
    /// - immutably on every [`Conn`](trillium::Conn) as
    ///   [`Conn::shared_state`](trillium::Conn::shared_state)
    /// - immutably on the [`BoundInfo`](crate::BoundInfo) as
    ///   [`BoundInfo::shared_state`](crate::BoundInfo::shared_state) returned by
    ///   [`ServerHandle::info`](crate::ServerHandle::info)
    pub fn set_shared_state<T: Send + Sync + 'static>(&mut self, state: T) -> &mut Self {
        self.context.shared_state_mut().insert(state);
        self
    }
}

impl<ServerType: Server> Config<ServerType, ()> {
    /// build a new config with default acceptor
    pub fn new() -> Self {
        Self::default()
    }

    /// Upgrade this single-listener configuration into a multi-listener [`ListenerConfig`],
    /// carrying over the global server configuration — HTTP config, shared state, [`Swansong`],
    /// `nodelay`, max-connections, and signal handling — but no listener binding. Bind one or
    /// more listeners explicitly on the returned builder
    /// ([`bind_tcp`](ListenerConfig::bind_tcp), [`bind_tls`](ListenerConfig::bind_tls),
    /// [`bind_quic`](ListenerConfig::bind_quic), [`bind_env`](ListenerConfig::bind_env), …).
    ///
    /// This is available before an acceptor or QUIC configuration is set; in the multi-listener
    /// model those are per-listener (`bind_tls`/`bind_quic`) rather than server-global. Any
    /// host/port set with [`with_host`](Self::with_host)/[`with_port`](Self::with_port), or a
    /// prebound server from [`with_prebound_server`](Self::with_prebound_server), is not carried
    /// over — binding on the builder is always explicit — and a warning is logged if one was set.
    pub fn listeners(self) -> ListenerConfig<ServerType> {
        if self.host.is_some() || self.port.is_some() || self.binding.is_some() {
            log::warn!(
                "Config::listeners() does not carry over host/port/prebound-server configuration; \
                 bind listeners explicitly on the returned ListenerConfig"
            );
        }
        ListenerConfig::from_global(
            self.context,
            self.context_cell,
            self.runtime,
            self.max_connections,
            self.nodelay,
            self.register_signals,
        )
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

pub(crate) fn info_with_server_header<ServerType: Server>(
    context: HttpContext,
    runtime: &ServerType::Runtime,
) -> Info {
    let mut info = Info::from(context)
        .with_shared_state(runtime.clone().into())
        .with_shared_state(runtime.clone());

    info.shared_state_entry::<Headers>()
        .or_default()
        .try_insert(KnownHeaderName::Server, trillium::headers::server_header());

    info
}

/// Acceptor-independent one-time server initialization: given an `Info` whose bound address has
/// already been populated, bind QUIC if configured, run [`Handler::init`] exactly once, and produce
/// the `Arc`-shared [`HttpContext`] and handler that any number of per-acceptor accept loops can
/// share.
///
/// `is_secure` governs URL-scheme derivation and is supplied by the caller rather than read from an
/// acceptor, because a single shared handler may front listeners with differing security (e.g. a
/// plaintext listener alongside a TLS one).
#[cfg_attr(not(unix), allow(unused_mut))]
pub(crate) async fn init_shared<ServerType, QuicType, H>(
    mut info: Info,
    runtime: ServerType::Runtime,
    quic: QuicType,
    mut max_connections: Option<usize>,
    is_secure: bool,
    mut handler: H,
) -> (
    Arc<HttpContext>,
    ArcHandler<H>,
    Option<QuicType::Endpoint>,
    Option<usize>,
)
where
    ServerType: Server,
    QuicType: QuicConfig<ServerType>,
    H: Handler,
{
    #[cfg(unix)]
    if max_connections.is_none() {
        max_connections = rlimit::getrlimit(rlimit::Resource::NOFILE)
            .ok()
            .and_then(|(soft, _hard)| soft.try_into().ok())
            .map(|limit: usize| ((limit as f32) * 0.75) as usize);
    }

    log::debug!("using max connections of {max_connections:?}");

    let quic_binding = if let Some(socket_addr) = info.tcp_socket_addr().copied() {
        let quic_binding = quic
            .bind(socket_addr, runtime, &mut info)
            .map(|r| r.expect("failed to bind QUIC endpoint"));

        if quic_binding.is_some() {
            info.shared_state_entry::<Headers>()
                .or_default()
                .try_insert_with(KnownHeaderName::AltSvc, || -> &'static str {
                    format!("h3=\":{}\"", socket_addr.port()).leak()
                });
        }

        quic_binding
    } else {
        None
    };

    // Populate the single-listener server's listener set, unless a multi-listener builder already
    // installed the full set. Read the addresses out before mutating, to avoid overlapping borrows.
    if info.shared_state::<Listeners>().is_none()
        && let Some(primary) = primary_listener(&info, is_secure)
    {
        let mut listeners = vec![primary];
        if quic_binding.is_some()
            && let Some(addr) = info.tcp_socket_addr().copied()
        {
            listeners.push(Listener::quic(addr));
        }
        info.insert_shared_state(Listeners(listeners));
    }

    insert_url(info.as_mut(), is_secure);

    handler.init(&mut info).await;

    let context = Arc::new(HttpContext::from(info));
    let handler = ArcHandler::new(handler);

    (context, handler, quic_binding, max_connections)
}

/// Spawn the OS-signal graceful-shutdown handler onto `runtime` if `register` is set. Standalone so
/// multi-listener flows can register signals once regardless of which (or whether any) TCP or QUIC
/// listener is involved.
pub(crate) fn spawn_signals_loop<R: RuntimeTrait>(
    context: Arc<HttpContext>,
    register: bool,
    runtime: R,
) {
    if !register {
        return;
    }
    let swansong = context.swansong().clone();
    runtime.clone().spawn(async move {
        let mut signals = pin!(runtime.hook_signals([2, 3, 15]));
        while signals.next().await.is_some() {
            let guard_count = swansong.guard_count();
            if swansong.state().is_shutting_down() {
                eprintln!(
                    "\nSecond interrupt, shutting down harshly (dropping {guard_count} guards)"
                );
                std::process::exit(1);
            } else {
                println!(
                    "\nShutting down gracefully. Waiting for {guard_count} shutdown guards to \
                     drop.\nControl-c again to force."
                );
                swansong.shut_down();
            }
        }
    });
}

/// The single-listener server's primary [`Listener`] — its TCP listener, or on unix its
/// Unix-domain listener — derived from the addresses `Server::init` populated. `None` if neither is
/// present (e.g. a not-yet-bound config).
pub(crate) fn primary_listener(info: &Info, is_secure: bool) -> Option<Listener> {
    if let Some(addr) = info.tcp_socket_addr().copied() {
        return Some(Listener::tcp(addr, is_secure));
    }
    #[cfg(unix)]
    if let Some(path) = info
        .unix_socket_addr()
        .and_then(|addr| addr.as_pathname().map(std::path::Path::to_path_buf))
    {
        return Some(Listener::unix(Some(path), is_secure));
    }
    None
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
