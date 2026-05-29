use crate::{
    Acceptor, ArcedQuicEndpoint, BoxedAcceptor, BoxedQuicConfig, QuicConfig, RuntimeTrait, Server,
    ServerHandle, SharedServer,
    config::{info_with_server_header, init_shared},
    shared_server::spawn_signals_loop,
};
use async_cell::sync::AsyncCell;
use std::{
    cell::OnceCell,
    collections::HashMap,
    fmt::{self, Debug, Formatter},
    future::Future,
    io,
    net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener, UdpSocket as StdUdpSocket},
    pin::Pin,
    sync::Arc,
    thread::JoinHandle,
};
use trillium::{BoundTcpAddrs, Handler, HttpConfig, Swansong};
use trillium_http::HttpContext;

/// A worker body, type-erased over the handler, that builds and drives a reuseport listener group's
/// accept loops for one worker index. Built inside [`ServerBuilder::run_async`] (where the handler
/// type is known) and invoked once per worker thread.
type BoxedWorker = Arc<dyn Fn(usize) -> Pin<Box<dyn Future<Output = ()>>> + Send + Sync>;

/// A monomorphized [`FanOut::thread_per_core`](crate::FanOut::thread_per_core) call for a concrete
/// runtime, erased to a function pointer. Stored on the builder only when a `bind_reuseport_*`
/// method runs, so the `FanOut` bound is discharged at the bind site while `run_async` stays free
/// of it (the handler type isn't known until `spawn`).
type ThreadPerCoreInvoker<R> = fn(&R, usize, BoxedWorker) -> Vec<JoinHandle<()>>;

/// Conversion into a TCP bind address. Implemented for a bare port (binds `0.0.0.0:port`) and for a
/// full [`SocketAddr`] (so an admin listener can bind a specific interface).
pub trait IntoListenAddr {
    /// Resolve to the concrete socket address to bind.
    fn into_listen_addr(self) -> SocketAddr;
}

impl IntoListenAddr for SocketAddr {
    fn into_listen_addr(self) -> SocketAddr {
        self
    }
}

impl IntoListenAddr for u16 {
    fn into_listen_addr(self) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, self))
    }
}

/// A multi-listener server builder.
///
/// Holds the inputs shared across every listener — handler (supplied at [`spawn`](Self::spawn)),
/// swansong, shared state, HTTP config — and a set of registered listeners. Each `bind_*` method
/// claims its address eagerly (failing fast) and the listeners are adopted into the runtime when the
/// server is spawned, after [`Handler::init`] has run exactly once across all of them.
///
/// This is the runtime-agnostic core; runtime adapters expose a constructor (e.g.
/// `trillium_tokio::server()`).
pub struct ServerBuilder<ServerType: Server> {
    context: HttpContext,
    context_cell: Arc<AsyncCell<Arc<HttpContext>>>,
    runtime: ServerType::Runtime,
    max_connections: Option<usize>,
    nodelay: bool,
    register_signals: bool,
    listeners: Vec<(StdTcpListener, BoxedAcceptor<ServerType::Transport>)>,
    quic_listeners: Vec<(StdUdpSocket, BoxedQuicConfig<ServerType>)>,
    alt_svc_pairs: Vec<(u16, u16)>,
    reuseport_listeners: Vec<(SocketAddr, StdTcpListener, BoxedAcceptor<ServerType::Transport>)>,
    reuseport_workers: Option<usize>,
    thread_per_core: Option<ThreadPerCoreInvoker<ServerType::Runtime>>,
}

impl<ServerType: Server> Debug for ServerBuilder<ServerType> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerBuilder")
            .field("max_connections", &self.max_connections)
            .field("nodelay", &self.nodelay)
            .field("register_signals", &self.register_signals)
            .field("listeners", &self.listeners)
            .field("quic_listeners", &self.quic_listeners)
            .field("alt_svc_pairs", &self.alt_svc_pairs)
            .field("reuseport_listeners", &self.reuseport_listeners)
            .field("reuseport_workers", &self.reuseport_workers)
            .finish_non_exhaustive()
    }
}

impl<ServerType: Server> Default for ServerBuilder<ServerType> {
    fn default() -> Self {
        Self {
            context: HttpContext::default(),
            context_cell: AsyncCell::shared(),
            runtime: ServerType::runtime(),
            max_connections: None,
            nodelay: false,
            register_signals: cfg!(unix),
            listeners: Vec::new(),
            quic_listeners: Vec::new(),
            alt_svc_pairs: Vec::new(),
            reuseport_listeners: Vec::new(),
            reuseport_workers: None,
            thread_per_core: None,
        }
    }
}

impl<ServerType: Server> ServerBuilder<ServerType> {
    /// Construct a new builder with the default runtime and no listeners.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a plaintext TCP listener. The address is bound immediately; an error here means the
    /// bind failed (e.g. the port is in use). Binding to port `0` selects an ephemeral port, which
    /// will be reflected in the bound address reported after [`spawn`](Self::spawn).
    pub fn bind_tcp(mut self, addr: impl IntoListenAddr) -> io::Result<Self> {
        let listener = StdTcpListener::bind(addr.into_listen_addr())?;
        listener.set_nonblocking(true)?;
        self.listeners.push((listener, BoxedAcceptor::new(())));
        Ok(self)
    }

    /// Register a TLS listener, terminating TLS with the provided acceptor (e.g. from
    /// `trillium-rustls` or `trillium-native-tls`). The protocols offered (h1, h2) follow the
    /// acceptor's ALPN configuration. Like [`bind_tcp`](Self::bind_tcp), the address is bound
    /// immediately.
    pub fn bind_tls<A>(mut self, addr: impl IntoListenAddr, acceptor: A) -> io::Result<Self>
    where
        A: Acceptor<ServerType::Transport>,
    {
        let listener = StdTcpListener::bind(addr.into_listen_addr())?;
        listener.set_nonblocking(true)?;
        self.listeners.push((listener, BoxedAcceptor::new(acceptor)));
        Ok(self)
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
        let socket = StdUdpSocket::bind(addr.into_listen_addr())?;
        socket.set_nonblocking(true)?;
        self.quic_listeners.push((socket, BoxedQuicConfig::new(quic)));
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

    /// Use the provided [`Swansong`] for graceful shutdown coordination.
    pub fn with_swansong(mut self, swansong: Swansong) -> Self {
        self.context.set_swansong(swansong);
        self
    }

    /// Add a value to the shared state [`TypeSet`](trillium::TypeSet), available on
    /// [`Info`](trillium::Info) during [`Handler::init`] and on every [`Conn`](trillium::Conn).
    pub fn with_shared_state<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.context.shared_state_mut().insert(state);
        self
    }

    /// Configure trillium-http performance and security tuning parameters.
    pub fn with_http_config(mut self, config: HttpConfig) -> Self {
        *self.context.config_mut() = config;
        self
    }

    /// Configure the maximum number of concurrent connections across all listeners. The default is
    /// 75% of the soft `rlimit_nofile` on unix systems and `None` elsewhere.
    pub fn with_max_connections(mut self, max_connections: Option<usize>) -> Self {
        self.max_connections = max_connections;
        self
    }

    /// Enable `TCP_NODELAY` on accepted connections.
    pub fn with_nodelay(mut self) -> Self {
        self.nodelay = true;
        self
    }

    /// Do not register OS signal handlers for graceful shutdown.
    pub fn without_signals(mut self) -> Self {
        self.register_signals = false;
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

    /// Initialize the handler once and drive every registered listener's accept loop until shutdown.
    /// This is the appropriate entrypoint when embedding in an already-running runtime; see
    /// [`run`](Self::run) and [`spawn`](Self::spawn) for the terminal forms.
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

        let mut info = info_with_server_header::<ServerType>(context, &runtime);

        // Read each claimed TCP listener's resolved address (incl. `:0` ephemeral ports), then
        // adopt it into the runtime (we are inside the runtime here). We own address insertion
        // rather than calling `Server::init` per listener, because that inserts a single
        // `SocketAddr` (last wins) — instead we record the full set and designate a primary. For
        // TCP servers `Server::init` does nothing beyond inserting that address.
        let mut bound_addrs = Vec::with_capacity(listeners.len());
        let mut adopted = Vec::with_capacity(listeners.len());
        for (listener, acceptor) in listeners {
            let addr = match listener.local_addr() {
                Ok(addr) => {
                    bound_addrs.push(addr);
                    Some(addr)
                }
                Err(e) => {
                    log::warn!("could not read local_addr of a bound listener: {e}");
                    None
                }
            };
            adopted.push((ServerType::from_tcp(listener), acceptor, addr));
        }

        // Primary address drives URL-scheme derivation; the full set is exposed via
        // `Info::tcp_addrs` / `BoundInfo::tcp_addrs`. URL-scheme derivation reflects the primary
        // listener's security; per-connection security is each listener's own acceptor.
        // Reuseport listeners are TCP too — their resolved addresses join the bound set so they
        // appear in `tcp_addrs` and participate in alt-svc resolution, even though their accept
        // loops run on the worker fleet rather than the shared runtime.
        bound_addrs.extend(reuseport_listeners.iter().map(|(addr, _, _)| *addr));

        if let Some(primary) = bound_addrs.first().copied() {
            info.insert_shared_state(primary);
        }
        info.insert_shared_state(BoundTcpAddrs(bound_addrs));

        let primary_is_secure = adopted
            .first()
            .map(|(_, a, _)| a.is_secure())
            .or_else(|| reuseport_listeners.first().map(|(_, _, a)| a.is_secure()))
            .unwrap_or(false);

        // Adopt each pre-claimed UDP socket into the runtime via its `BoxedQuicConfig`. `&mut info`
        // gives quic adapters the same affordance as `Config`'s single-listener path. Endpoint
        // construction happens BEFORE `init_shared` so `Handler::init` sees any state the adapter
        // populates and so we know each quic port for alt-svc resolution.
        let mut adopted_quic: Vec<(ArcedQuicEndpoint, Option<SocketAddr>)> =
            Vec::with_capacity(quic_listeners.len());
        for (socket, boxed_quic) in quic_listeners {
            let local_addr = socket.local_addr().ok();
            match boxed_quic.bind(socket, runtime.clone(), &mut info) {
                Ok(endpoint) => adopted_quic.push((endpoint, local_addr)),
                Err(e) => log::error!("failed to bind QUIC endpoint at {local_addr:?}: {e}"),
            }
        }

        let listener_ports: Vec<u16> = adopted
            .iter()
            .filter_map(|(_, _, addr)| addr.as_ref().map(SocketAddr::port))
            .chain(reuseport_listeners.iter().map(|(addr, _, _)| addr.port()))
            .collect();
        let quic_ports: Vec<u16> = adopted_quic
            .iter()
            .filter_map(|(_, addr)| addr.as_ref().map(SocketAddr::port))
            .collect();

        let alt_svc_strs = build_alt_svc_map(&listener_ports, &quic_ports, &alt_svc_pairs);

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

        // Spawn the reuseport worker fleet (if any). The join handles are dropped — workers are
        // detached and drain via the shared swansong, like every other accept loop.
        #[cfg(all(
            unix,
            not(target_os = "solaris"),
            not(target_os = "illumos"),
            not(target_os = "cygwin"),
            not(target_vendor = "apple")
        ))]
        if let Some(invoke) = thread_per_core {
            let _workers = spawn_reuseport_fleet::<ServerType, _>(
                invoke,
                reuseport_listeners,
                reuseport_workers,
                &runtime,
                context.clone(),
                handler.clone(),
                max_connections,
                nodelay,
                &alt_svc_strs,
            );
        }
        #[cfg(not(all(
            unix,
            not(target_os = "solaris"),
            not(target_os = "illumos"),
            not(target_os = "cygwin"),
            not(target_vendor = "apple")
        )))]
        let _ = (thread_per_core, reuseport_listeners, reuseport_workers);

        // Each TCP listener gets its own `SharedServer` carrying its (erased) acceptor, bound
        // address, and pre-resolved `alt-svc` value, all sharing the single initialized context +
        // handler.
        let alt_svc_strs_ref = &alt_svc_strs;
        let new_shared =
            |acceptor: BoxedAcceptor<ServerType::Transport>, local_addr: Option<SocketAddr>| {
                let local_alt_svc =
                    local_addr.and_then(|a| alt_svc_strs_ref.get(&a.port()).copied());
                SharedServer::<ServerType, _, _>::new(
                    acceptor,
                    max_connections,
                    nodelay,
                    local_addr,
                    local_alt_svc,
                    false,
                    context.clone(),
                    handler.clone(),
                )
            };

        let mut joins = Vec::new();

        // Spawn one h3 accept loop per quic endpoint. The futures are pin-boxed up front so the
        // Vec's element type is uniform across h3 and TCP spawns (each `async move {…}` is a
        // distinct anonymous type otherwise).
        for (endpoint, local_addr) in adopted_quic {
            let local_alt_svc = local_addr.and_then(|a| alt_svc_strs.get(&a.port()).copied());
            let ctx = context.clone();
            let h = handler.clone();
            let rt = runtime.clone();
            let fut: Pin<Box<dyn Future<Output = ()> + Send>> = Box::pin(async move {
                crate::h3::run_h3(endpoint, ctx, h, rt, local_addr, local_alt_svc).await;
            });
            joins.push(runtime.clone().spawn(fut));
        }

        // Run the first TCP accept loop inline (so this future lives until shutdown); spawn the
        // rest. If there are no TCP listeners, fall back to awaiting all QUIC loops; if there are
        // no listeners of either kind, just hold open until shutdown.
        let mut adopted = adopted.into_iter();
        let Some((first_server, first_acceptor, first_addr)) = adopted.next() else {
            if joins.is_empty() {
                log::warn!("server spawned with no listeners; awaiting shutdown");
                context.swansong().clone().await;
            } else {
                for join in joins {
                    join.await;
                }
            }
            return;
        };

        let first_shared = new_shared(first_acceptor, first_addr);

        for (server, acceptor, addr) in adopted {
            let shared = new_shared(acceptor, addr);
            let rt = runtime.clone();
            let fut: Pin<Box<dyn Future<Output = ()> + Send>> = Box::pin(async move {
                shared.accept_loop(rt.clone(), server).await;
            });
            joins.push(runtime.clone().spawn(fut));
        }

        first_shared.accept_loop(runtime, first_server).await;
        for join in joins {
            join.await;
        }
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

/// Reuseport thread-per-core binds, available only on platforms where `SO_REUSEPORT` actually
/// load-balances connections (Linux and other non-Apple, non-Solaris/illumos/cygwin Unixes) and
/// only for runtimes whose [`Runtime`](crate::Runtime) implements [`FanOut`](crate::FanOut). The
/// trait bound is the gate: a runtime without a `FanOut` impl simply does not have these methods.
#[cfg(all(
    unix,
    not(target_os = "solaris"),
    not(target_os = "illumos"),
    not(target_os = "cygwin"),
    not(target_vendor = "apple")
))]
impl<ServerType: Server> ServerBuilder<ServerType>
where
    ServerType::Runtime: crate::FanOut,
{
    /// Register a plaintext TCP listener fanned across per-core worker threads.
    ///
    /// Unlike [`bind_tcp`](Self::bind_tcp), which adopts one listener onto the shared
    /// multi-threaded runtime, this binds one `SO_REUSEPORT` listener per worker thread into a
    /// kernel reuseport group and runs each listener's accept loop on its own single-threaded
    /// executor — keeping each connection's work on the core the kernel delivered it to. The
    /// address is claimed immediately (resolving `:0` to the group's shared port), failing fast.
    pub fn bind_reuseport_tcp(self, addr: impl IntoListenAddr) -> io::Result<Self> {
        self.bind_reuseport(addr.into_listen_addr(), BoxedAcceptor::new(()))
    }

    /// Register a TLS listener fanned across per-core worker threads. The reuseport equivalent of
    /// [`bind_tls`](Self::bind_tls); see [`bind_reuseport_tcp`](Self::bind_reuseport_tcp) for the
    /// fan-out semantics.
    pub fn bind_reuseport_tls<A>(self, addr: impl IntoListenAddr, acceptor: A) -> io::Result<Self>
    where
        A: Acceptor<ServerType::Transport>,
    {
        self.bind_reuseport(addr.into_listen_addr(), BoxedAcceptor::new(acceptor))
    }

    /// Set the number of per-core worker threads for reuseport listeners. Defaults to the `WORKERS`
    /// environment variable, falling back to the available parallelism.
    pub fn with_reuseport_workers(mut self, workers: usize) -> Self {
        self.reuseport_workers = Some(workers);
        self
    }

    fn bind_reuseport(
        mut self,
        addr: SocketAddr,
        acceptor: BoxedAcceptor<ServerType::Transport>,
    ) -> io::Result<Self> {
        let listener = crate::bind_reuse_port(addr)?;
        let resolved = listener.local_addr()?;
        self.reuseport_listeners.push((resolved, listener, acceptor));
        self.thread_per_core = Some(invoke_thread_per_core::<ServerType::Runtime>);
        Ok(self)
    }
}

/// Invoke [`FanOut::thread_per_core`](crate::FanOut::thread_per_core) on a concrete runtime with a
/// type-erased worker. Coerced to a [`ThreadPerCoreInvoker`] function pointer at the
/// `bind_reuseport_*` call site, where the `FanOut` bound is available.
#[cfg(all(
    unix,
    not(target_os = "solaris"),
    not(target_os = "illumos"),
    not(target_os = "cygwin"),
    not(target_vendor = "apple")
))]
fn invoke_thread_per_core<R: crate::FanOut>(
    runtime: &R,
    count: usize,
    worker: BoxedWorker,
) -> Vec<JoinHandle<()>> {
    runtime.thread_per_core(count, move |idx| worker(idx))
}

/// Spawn the per-core worker fleet for a server's reuseport listeners. Each worker runs one accept
/// loop per reuseport bind on its own single-threaded executor; worker 0 adopts the listener
/// claimed eagerly at bind time, and the rest bind fresh members into the kernel reuseport group.
/// The returned join handles are detached by the caller — workers drain via the shared
/// [`Swansong`].
#[cfg(all(
    unix,
    not(target_os = "solaris"),
    not(target_os = "illumos"),
    not(target_os = "cygwin"),
    not(target_vendor = "apple")
))]
#[allow(clippy::too_many_arguments)]
fn spawn_reuseport_fleet<ServerType: Server, H: Handler>(
    invoke: ThreadPerCoreInvoker<ServerType::Runtime>,
    listeners: Vec<(SocketAddr, StdTcpListener, BoxedAcceptor<ServerType::Transport>)>,
    workers: Option<usize>,
    runtime: &ServerType::Runtime,
    context: Arc<HttpContext>,
    handler: crate::ArcHandler<H>,
    max_connections: Option<usize>,
    nodelay: bool,
    alt_svc: &HashMap<u16, &'static str>,
) -> Vec<JoinHandle<()>> {
    let count = workers
        .or_else(|| std::env::var("WORKERS").ok().and_then(|w| w.parse().ok()))
        .unwrap_or_else(available_parallelism)
        .max(1);

    // Pre-claimed listeners go into a take-once cell each: worker 0 adopts them (so the port
    // claimed at bind time is never released), every other worker binds a fresh group member.
    let binds: Vec<_> = listeners
        .into_iter()
        .map(|(addr, listener, acceptor)| {
            let local_alt_svc = alt_svc.get(&addr.port()).copied();
            (
                addr,
                Arc::new(std::sync::Mutex::new(Some(listener))),
                acceptor,
                local_alt_svc,
            )
        })
        .collect();
    let binds = Arc::new(binds);

    log::info!(
        "reuseport: {count} per-core worker(s) across {} listener(s)",
        binds.len()
    );

    let worker: BoxedWorker = Arc::new(move |idx: usize| {
        let binds = Arc::clone(&binds);
        let context = context.clone();
        let handler = handler.clone();
        Box::pin(async move {
            let runtime = ServerType::runtime();
            let mut loops = Vec::with_capacity(binds.len());
            for (addr, claimed, acceptor, local_alt_svc) in binds.iter() {
                let listener = if idx == 0 {
                    claimed.lock().unwrap().take()
                } else {
                    None
                };
                let listener = match listener {
                    Some(listener) => listener,
                    None => match crate::bind_reuse_port(*addr) {
                        Ok(listener) => listener,
                        Err(e) => {
                            log::error!("reuseport worker {idx}: could not bind {addr}: {e}");
                            continue;
                        }
                    },
                };
                let shared = SharedServer::<ServerType, _, H>::new(
                    acceptor.clone(),
                    max_connections,
                    nodelay,
                    Some(*addr),
                    *local_alt_svc,
                    false,
                    context.clone(),
                    handler.clone(),
                );
                let server = ServerType::from_tcp(listener);
                let accept_runtime = runtime.clone();
                loops.push(
                    runtime.spawn(async move { shared.accept_loop(accept_runtime, server).await }),
                );
            }
            for accept_loop in loops {
                accept_loop.await;
            }
        }) as Pin<Box<dyn Future<Output = ()>>>
    });

    invoke(runtime, count, worker)
}

#[cfg(all(
    unix,
    not(target_os = "solaris"),
    not(target_os = "illumos"),
    not(target_os = "cygwin"),
    not(target_vendor = "apple")
))]
fn available_parallelism() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}

/// Resolve `bind_quic` / `with_alt_svc` declarations into `from_port → &'static str` alt-svc
/// header values. A TCP/TLS listener and a quic listener bound to the same port auto-pair; any
/// non-matching pairing must be added with `with_alt_svc`. Values sharing a `from` port are merged
/// into one comma-joined header value, leaked once so it can be shared cheaply across responses
/// and is eligible for h2/h3 dynamic-table reuse.
///
/// Dangling references in `with_alt_svc` pairs (from-port not bound TCP, or to-port not bound
/// QUIC) are warned but otherwise included; the user knows their topology better than we do.
fn build_alt_svc_map(
    listener_ports: &[u16],
    quic_ports: &[u16],
    explicit_pairs: &[(u16, u16)],
) -> HashMap<u16, &'static str> {
    let mut pairs_by_from: HashMap<u16, Vec<u16>> = HashMap::new();

    for &p in quic_ports {
        if listener_ports.contains(&p) {
            pairs_by_from.entry(p).or_default().push(p);
        }
    }

    for &(from, to) in explicit_pairs {
        if !listener_ports.contains(&from) {
            log::warn!("with_alt_svc({from}, {to}): no TCP listener bound on port {from}");
        }
        if !quic_ports.contains(&to) {
            log::warn!("with_alt_svc({from}, {to}): no QUIC listener bound on port {to}");
        }
        let tos = pairs_by_from.entry(from).or_default();
        if !tos.contains(&to) {
            tos.push(to);
        }
    }

    pairs_by_from
        .into_iter()
        .map(|(from, tos)| {
            let value = tos
                .iter()
                .map(|t| format!("h3=\":{t}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let leaked: &'static str = Box::leak(value.into_boxed_str());
            (from, leaked)
        })
        .collect()
}
