use crate::{
    Acceptor, BoxedAcceptor, BoxedQuicConfig, QuicConfig, RuntimeTrait, Server, ServerHandle,
    config::{init_shared, spawn_signals_loop},
    server::PreboundListener,
};
use async_cell::sync::AsyncCell;
use std::{
    cell::OnceCell,
    fmt::{self, Debug, Formatter},
    future::Future,
    io,
    net::{SocketAddr, TcpListener as StdTcpListener, UdpSocket as StdUdpSocket},
    pin::Pin,
    sync::Arc,
    thread::JoinHandle,
};
#[cfg(unix)]
use std::{os::unix::net::UnixListener, path::Path};
use trillium::Handler;
use trillium_http::HttpContext;

mod helpers;
mod into_listen_addr;
#[cfg(reuseport)]
mod reuseport;
mod run;

use helpers::{resolve_env_listener, take_inherited_fd};
pub use into_listen_addr::IntoListenAddr;
use run::{Resolved, Shared};

/// A worker body, type-erased over the handler, that builds and drives a reuseport listener group's
/// accept loops for one worker index. Built inside [`ListenerConfig::run_async`] (where the handler
/// type is known) and invoked once per worker thread.
type BoxedWorker = Arc<dyn Fn(usize) -> Pin<Box<dyn Future<Output = ()>>> + Send + Sync>;

/// A monomorphized [`FanOut::thread_per_core`](crate::FanOut::thread_per_core) call for a concrete
/// runtime, erased to a function pointer. Stored on the builder only when a `bind_reuseport_*`
/// method runs, so the `FanOut` bound is discharged at the bind site while `run_async` stays free
/// of it (the handler type isn't known until `spawn`).
type ThreadPerCoreInvoker<R> = fn(&R, usize, BoxedWorker) -> Vec<JoinHandle<()>>;

/// A registered standard listener, before its accept loop is driven. Either a [`PreboundListener`]
/// the builder claimed itself (from `bind_tcp`/`bind_tls`/`bind_fd`/`bind_uds`/`bind_env`) and will
/// adopt into the runtime at spawn, or a [`Server`] the caller already bound and handed over via
/// [`bind_server`](ListenerConfig::bind_server) (the bridge for [`Config::with_prebound_server`]).
pub(super) enum ListenerSource<ServerType> {
    Prebound(PreboundListener),
    Adopted(ServerType),
}

/// A reuseport listener registered on the builder: its resolved address (the group's shared port),
/// the first group member claimed eagerly at bind time, and its erased acceptor.
type ReuseportListener<ServerType> = (
    SocketAddr,
    StdTcpListener,
    BoxedAcceptor<<ServerType as Server>::Transport>,
);

/// An advanced listener builder.
///
/// Holds the inputs shared across every listener — handler (supplied at [`spawn`](Self::spawn)),
/// swansong, shared state, HTTP config — and a set of registered listeners. Each `bind_*` method
/// claims its address eagerly (failing fast) and the listeners are adopted into the runtime when
/// the server is spawned, after [`Handler::init`] has run exactly once across all of them.
///
/// Construct one from a [`Config`](crate::Config) with
/// [`Config::listeners`](crate::Config::listeners): the global server configuration (swansong,
/// shared state, HTTP config, nodelay, max-connections, signals) is set on the `Config`, then
/// carried over; the builder adds listener topology.
pub struct ListenerConfig<ServerType: Server> {
    context: HttpContext,
    context_cell: Arc<AsyncCell<Arc<HttpContext>>>,
    runtime: ServerType::Runtime,
    max_connections: Option<usize>,
    nodelay: bool,
    register_signals: bool,
    listeners: Vec<(
        ListenerSource<ServerType>,
        BoxedAcceptor<ServerType::Transport>,
    )>,
    quic_listeners: Vec<(StdUdpSocket, BoxedQuicConfig<ServerType>)>,
    alt_svc_pairs: Vec<(u16, u16)>,
    reuseport_listeners: Vec<ReuseportListener<ServerType>>,
    reuseport_workers: Option<usize>,
    thread_per_core: Option<ThreadPerCoreInvoker<ServerType::Runtime>>,
}

impl<ServerType: Server> Debug for ListenerConfig<ServerType> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ListenerConfig")
            .field("max_connections", &self.max_connections)
            .field("nodelay", &self.nodelay)
            .field("register_signals", &self.register_signals)
            .field("listeners", &self.listeners.len())
            .field("quic_listeners", &self.quic_listeners)
            .field("alt_svc_pairs", &self.alt_svc_pairs)
            .field("reuseport_listeners", &self.reuseport_listeners)
            .field("reuseport_workers", &self.reuseport_workers)
            .finish_non_exhaustive()
    }
}

impl<ServerType: Server> ListenerConfig<ServerType> {
    /// Construct a builder carrying the given global server configuration and no listeners.
    pub(crate) fn from_global(
        context: HttpContext,
        context_cell: Arc<AsyncCell<Arc<HttpContext>>>,
        runtime: ServerType::Runtime,
        max_connections: Option<usize>,
        nodelay: bool,
        register_signals: bool,
    ) -> Self {
        Self {
            context,
            context_cell,
            runtime,
            max_connections,
            nodelay,
            register_signals,
            listeners: Vec::new(),
            quic_listeners: Vec::new(),
            alt_svc_pairs: Vec::new(),
            reuseport_listeners: Vec::new(),
            reuseport_workers: None,
            thread_per_core: None,
        }
    }

    /// Register a plaintext TCP listener. The address is resolved and bound immediately; an error
    /// means the address could not be resolved or the bind failed (e.g. the port is in use).
    /// Binding to port `0` selects an ephemeral port, which will be reflected in the bound
    /// address reported after [`spawn`](Self::spawn).
    pub fn bind_tcp(self, addr: impl IntoListenAddr) -> io::Result<Self> {
        let listener = StdTcpListener::bind(addr.into_listen_addr()?)?;
        listener.set_nonblocking(true)?;
        Ok(self.push_listener(PreboundListener::Tcp(listener), BoxedAcceptor::new(())))
    }

    /// Register a TLS listener, terminating TLS with the provided acceptor (e.g. from
    /// `trillium-rustls` or `trillium-native-tls`). The protocols offered (h1, h2) follow the
    /// acceptor's ALPN configuration. Like [`bind_tcp`](Self::bind_tcp), the address is resolved
    /// and bound immediately.
    pub fn bind_tls<A>(self, addr: impl IntoListenAddr, acceptor: A) -> io::Result<Self>
    where
        A: Acceptor<ServerType::Transport>,
    {
        let listener = StdTcpListener::bind(addr.into_listen_addr()?)?;
        listener.set_nonblocking(true)?;
        Ok(self.push_listener(
            PreboundListener::Tcp(listener),
            BoxedAcceptor::new(acceptor),
        ))
    }

    /// Register a plaintext TCP listener inherited from the environment as the file descriptor at
    /// `index` (as passed by a socket-activation supervisor such as systemfd or systemd, via the
    /// `LISTEN_FDS` protocol). The descriptor is claimed immediately; an error here means no
    /// inherited TCP listener was present at that index.
    ///
    /// Unlike the single-listener [`Config`](crate::Config) path, which auto-detects `LISTEN_FD`,
    /// this is explicit: each inherited descriptor is bound by index, so several can be adopted.
    pub fn bind_fd(self, index: usize) -> io::Result<Self> {
        let listener = take_inherited_fd(index)?;
        listener.set_nonblocking(true)?;
        Ok(self.push_listener(PreboundListener::Tcp(listener), BoxedAcceptor::new(())))
    }

    /// Register a single listener resolved from the environment, following trillium's 12-factor
    /// conventions: `HOST` (default `localhost`) and `PORT` (default `8080`), a listener inherited
    /// via the `LISTEN_FDS` socket-activation protocol if present, and — on unix — a `HOST`
    /// beginning with `/`, `.`, or `~` treated as a Unix-domain-socket path (the port is ignored).
    ///
    /// This is the explicit, fallible equivalent of the implicit binding that
    /// [`Config`](crate::Config) performs when no listener is configured. The listener is
    /// plaintext; for a TLS listener resolved the same way use
    /// [`bind_env_tls`](Self::bind_env_tls).
    pub fn bind_env(self) -> io::Result<Self> {
        Ok(self.push_listener(resolve_env_listener()?, BoxedAcceptor::new(())))
    }

    /// Register a single TLS listener resolved from the environment, terminating TLS with the
    /// provided acceptor. The TLS equivalent of [`bind_env`](Self::bind_env); see it for the
    /// resolution rules.
    pub fn bind_env_tls<A>(self, acceptor: A) -> io::Result<Self>
    where
        A: Acceptor<ServerType::Transport>,
    {
        Ok(self.push_listener(resolve_env_listener()?, BoxedAcceptor::new(acceptor)))
    }

    /// Register a TLS listener over a TCP descriptor inherited from the environment at `index`; see
    /// [`bind_fd`](Self::bind_fd) for how the descriptor is claimed.
    pub fn bind_fd_tls<A>(self, index: usize, acceptor: A) -> io::Result<Self>
    where
        A: Acceptor<ServerType::Transport>,
    {
        let listener = take_inherited_fd(index)?;
        listener.set_nonblocking(true)?;
        Ok(self.push_listener(
            PreboundListener::Tcp(listener),
            BoxedAcceptor::new(acceptor),
        ))
    }

    /// Register a plaintext listener on a Unix-domain socket at `path`. The socket is bound
    /// immediately; an error here means the bind failed (e.g. the path already exists).
    #[cfg(unix)]
    pub fn bind_uds(self, path: impl AsRef<Path>) -> io::Result<Self> {
        let listener = UnixListener::bind(path)?;
        listener.set_nonblocking(true)?;
        Ok(self.push_listener(PreboundListener::Unix(listener), BoxedAcceptor::new(())))
    }

    /// Register a TLS listener on a Unix-domain socket at `path`.
    #[cfg(unix)]
    pub fn bind_uds_tls<A>(self, path: impl AsRef<Path>, acceptor: A) -> io::Result<Self>
    where
        A: Acceptor<ServerType::Transport>,
    {
        let listener = UnixListener::bind(path)?;
        listener.set_nonblocking(true)?;
        Ok(self.push_listener(
            PreboundListener::Unix(listener),
            BoxedAcceptor::new(acceptor),
        ))
    }

    pub(crate) fn push_listener(
        mut self,
        listener: PreboundListener,
        acceptor: BoxedAcceptor<ServerType::Transport>,
    ) -> Self {
        self.listeners
            .push((ListenerSource::Prebound(listener), acceptor));
        self
    }

    /// Register a pre-claimed UDP socket and its QUIC configuration; the public entry point that
    /// claims the socket itself is [`bind_quic`](Self::bind_quic).
    pub(crate) fn push_quic_listener(
        mut self,
        socket: StdUdpSocket,
        quic: BoxedQuicConfig<ServerType>,
    ) -> Self {
        self.quic_listeners.push((socket, quic));
        self
    }

    /// Adopt a server the caller has already bound into the runtime (the multi-listener equivalent
    /// of [`Config::with_prebound_server`](crate::Config::with_prebound_server)). Plaintext; for a
    /// TLS-terminating prebound server use [`bind_server_tls`](Self::bind_server_tls).
    ///
    /// Infallible — there is no bind to fail. The adopted server's bound address is discovered by
    /// running its [`Server::init`] hook at spawn.
    pub fn bind_server(self, server: impl Into<ServerType>) -> Self {
        self.bind_server_boxed(server.into(), BoxedAcceptor::new(()))
    }

    /// Adopt an already-bound server terminating TLS with the provided acceptor. The
    /// TLS equivalent of [`bind_server`](Self::bind_server).
    pub fn bind_server_tls<A>(self, server: impl Into<ServerType>, acceptor: A) -> Self
    where
        A: Acceptor<ServerType::Transport>,
    {
        self.bind_server_boxed(server.into(), BoxedAcceptor::new(acceptor))
    }

    /// Adopt an already-bound server with a pre-erased acceptor.
    pub(crate) fn bind_server_boxed(
        mut self,
        server: ServerType,
        acceptor: BoxedAcceptor<ServerType::Transport>,
    ) -> Self {
        self.listeners
            .push((ListenerSource::Adopted(server), acceptor));
        self
    }

    /// Register a QUIC listener for HTTP/3, using the provided QUIC configuration (e.g. from
    /// `trillium-quinn`). The address's UDP socket is claimed immediately for fail-fast binding;
    /// the QUIC endpoint itself is constructed inside the runtime when the server is spawned.
    ///
    /// `bind_quic` does not by itself cause an `alt-svc` header to be advertised on any TCP
    /// listener. If a TCP or TLS listener is bound on the same port as this QUIC listener, the
    /// builder will auto-pair the two and advertise `alt-svc: h3=":<port>"` on that TCP
    /// listener's responses. Use [`with_alt_svc`](Self::with_alt_svc) to express any non-matching
    /// pairing (e.g. h3 on a different port from the TLS port that advertises it).
    pub fn bind_quic<Q>(mut self, addr: impl IntoListenAddr, quic: Q) -> io::Result<Self>
    where
        Q: QuicConfig<ServerType>,
    {
        let socket = StdUdpSocket::bind(addr.into_listen_addr()?)?;
        socket.set_nonblocking(true)?;
        self.quic_listeners
            .push((socket, BoxedQuicConfig::new(quic)));
        Ok(self)
    }

    /// Advertise an `alt-svc: h3=":<to>"` header on responses from the TCP/TLS listener bound to
    /// `from`, pointing at the QUIC listener bound to `to`. May be chained for multiple
    /// alternatives — values sharing a `from` port are merged into one header value.
    ///
    /// A matching same-port pair (a `bind_tcp(p)` or `bind_tls(p, _)` together with a
    /// `bind_quic(p, _)`) is auto-advertised without an explicit call; use this method only for
    /// pairings the builder cannot infer.
    ///
    /// `from` need not be a TLS port — emitting `alt-svc` from a plaintext listener is valid in
    /// topologies where something upstream (a TLS-terminating proxy or load balancer) provides
    /// the TLS view a client sees.
    pub fn with_alt_svc(mut self, from: u16, to: u16) -> Self {
        self.alt_svc_pairs.push((from, to));
        self
    }

    /// Return a [`ServerHandle`] for this builder, usable to await startup, retrieve the bound
    /// [`BoundInfo`](crate::BoundInfo), or initiate shutdown.
    pub fn handle(&self) -> ServerHandle {
        ServerHandle {
            swansong: self.context.swansong().clone(),
            context: self.context_cell.clone(),
            received_context: OnceCell::new(),
            runtime: self.runtime.clone().into(),
        }
    }

    /// Initialize the handler once and drive every registered listener's accept loop until
    /// shutdown. This is the appropriate entrypoint when embedding in an already-running
    /// runtime; see [`run`](Self::run) and [`spawn`](Self::spawn) for the terminal forms.
    pub async fn run_async(self, handler: impl Handler) {
        let Self {
            context,
            context_cell,
            runtime,
            max_connections,
            nodelay,
            register_signals,
            listeners,
            quic_listeners,
            alt_svc_pairs,
            reuseport_listeners,
            reuseport_workers,
            thread_per_core,
        } = self;

        // Adopt every registered listener into the runtime and fully populate `Info` — primary
        // address, public listener set, and resolved alt-svc — before any handler initialization.
        let Resolved {
            info,
            standard,
            quic,
            alt_svc,
            primary_is_secure,
        } = Resolved::resolve(
            context,
            &runtime,
            listeners,
            quic_listeners,
            &reuseport_listeners,
            &alt_svc_pairs,
        );

        // Run `Handler::init` exactly once and produce the `Arc`-shared context + handler that
        // every accept loop — shared-runtime, h3, and reuseport worker — drives.
        let (context, handler, _quic, max_connections) = init_shared::<ServerType, (), _>(
            info,
            runtime.clone(),
            (),
            max_connections,
            primary_is_secure,
            handler,
        )
        .await;

        context_cell.set(context.clone());
        spawn_signals_loop(context.clone(), register_signals, runtime.clone());

        let shared = Shared {
            context,
            handler,
            runtime,
            max_connections,
            nodelay,
            alt_svc,
        };

        // The reuseport fleet runs on its own per-core worker threads; the returned flag lets the
        // no-TCP-listener fallback in `drive` distinguish a reuseport-only server from one with
        // nothing bound.
        let has_reuseport_workers = shared.spawn_reuseport_fleet_if_configured(
            thread_per_core,
            reuseport_listeners,
            reuseport_workers,
        );

        shared.drive(standard, quic, has_reuseport_workers).await;
    }

    /// Spawn the server onto its runtime, returning a [`ServerHandle`] immediately.
    pub fn spawn(self, handler: impl Handler) -> ServerHandle {
        let handle = self.handle();
        let runtime = self.runtime.clone();
        runtime.spawn(self.run_async(handler));
        handle
    }

    /// Start the runtime and block on the server until shutdown.
    pub fn run(self, handler: impl Handler) {
        self.runtime.clone().block_on(self.run_async(handler));
    }
}
