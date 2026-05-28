//! Spawn-time machinery for [`ListenerConfig::run_async`](super::ListenerConfig::run_async), split
//! out from the builder API surface in the parent module.
//!
//! Two phases live here. [`Resolved::resolve`] adopts every registered listener into the runtime
//! and fully populates the shared [`Info`]. [`Shared`] then carries the one initialized context and
//! handler across all the accept loops it drives — shared-runtime TCP/Unix, h3, and (in
//! `reuseport.rs`) the per-core worker fleet.

#[cfg(not(reuseport))]
use super::ThreadPerCoreInvoker;
use super::{ListenerSource, ReuseportListener, helpers::build_alt_svc_map};
use crate::{
    Acceptor, ArcHandler, ArcedQuicEndpoint, BoxedAcceptor, BoxedQuicConfig, RuntimeTrait, Server,
    config::{info_with_server_header, primary_listener},
    running_config::RunningConfig,
    server::PreboundListener,
};
#[cfg(unix)]
use std::path::Path;
use std::{
    collections::HashMap,
    future::Future,
    net::{SocketAddr, UdpSocket as StdUdpSocket},
    sync::Arc,
};
use trillium::{Handler, Info, Listener, ListenerKind, Listeners};
use trillium_http::HttpContext;

/// A QUIC endpoint constructed at spawn time, paired with the resolved address it bound (`None` if
/// the socket's `local_addr` could not be read), used for the public listener set and alt-svc.
type AdoptedQuic = (ArcedQuicEndpoint, Option<SocketAddr>);

/// A standard (non-reuseport, non-QUIC) listener after adoption into the runtime: the concrete
/// [`Server`], its erased acceptor, and the public [`Listener`] record (`None` when an adopted
/// server's `init` inserted no address — e.g. a virtualized test server).
pub(super) struct AdoptedListener<ServerType: Server> {
    server: ServerType,
    acceptor: BoxedAcceptor<ServerType::Transport>,
    record: Option<Listener>,
}

/// The product of resolving every registered listener: each one adopted into the runtime, the QUIC
/// endpoints constructed, and the shared [`Info`] fully populated (primary address, public listener
/// set, server header). Ready to hand to `init_shared`.
pub(super) struct Resolved<ServerType: Server> {
    pub(super) info: Info,
    pub(super) standard: Vec<AdoptedListener<ServerType>>,
    pub(super) quic: Vec<AdoptedQuic>,
    pub(super) alt_svc: HashMap<u16, &'static str>,
    pub(super) primary_is_secure: bool,
}

impl<ServerType: Server> Resolved<ServerType> {
    pub(super) fn resolve(
        context: HttpContext,
        runtime: &ServerType::Runtime,
        listeners: Vec<(
            ListenerSource<ServerType>,
            BoxedAcceptor<ServerType::Transport>,
        )>,
        quic_listeners: Vec<(StdUdpSocket, BoxedQuicConfig<ServerType>)>,
        reuseport_listeners: &[ReuseportListener<ServerType>],
        alt_svc_pairs: &[(u16, u16)],
    ) -> Self {
        let mut info = info_with_server_header::<ServerType>(context, runtime);

        let (standard, mut bound_addrs) = adopt_standard_listeners(listeners, &mut info);

        // Reuseport listeners are TCP too — their resolved addresses join the bound set so they
        // appear in the listener set and participate in alt-svc resolution, even though their
        // accept loops run on the worker fleet rather than the shared runtime.
        bound_addrs.extend(reuseport_listeners.iter().map(|(addr, _, _)| *addr));

        // Designate the primary address ourselves rather than trusting the last-wins `SocketAddr`
        // each `Server::init` scribbles. It drives URL-scheme derivation; the full set is exposed
        // via `Info::listeners` / `BoundInfo::listeners`.
        if let Some(primary) = bound_addrs.first().copied() {
            info.insert_shared_state(primary);
        }

        // URL-scheme derivation reflects the primary listener's security; per-connection security
        // is each listener's own acceptor.
        let primary_is_secure = standard
            .first()
            .map(|l| l.acceptor.is_secure())
            .or_else(|| reuseport_listeners.first().map(|(_, _, a)| a.is_secure()))
            .unwrap_or(false);

        let quic = adopt_quic_listeners(quic_listeners, runtime, &mut info);

        let alt_svc = populate_listener_set(
            &mut info,
            &standard,
            reuseport_listeners,
            &quic,
            alt_svc_pairs,
        );

        Self {
            info,
            standard,
            quic,
            alt_svc,
            primary_is_secure,
        }
    }
}

/// Read each claimed listener's resolved address (TCP incl. `:0` ephemeral ports; Unix its path),
/// adopt it into the runtime (we are inside the runtime here), and run its `Server::init` hook for
/// any side effects a `Server` impl performs there. Returns the adopted listeners and their bound
/// addresses in registration order — the caller designates the first as primary.
fn adopt_standard_listeners<ServerType: Server>(
    listeners: Vec<(
        ListenerSource<ServerType>,
        BoxedAcceptor<ServerType::Transport>,
    )>,
    info: &mut Info,
) -> (Vec<AdoptedListener<ServerType>>, Vec<SocketAddr>) {
    let mut bound_addrs = Vec::with_capacity(listeners.len());
    let mut adopted = Vec::with_capacity(listeners.len());
    for (source, acceptor) in listeners {
        let secure = acceptor.is_secure();
        // A listener the builder claimed itself yields its record directly from the std listener's
        // resolved address. An adopted server is opaque, so its record is derived below from
        // whatever its `init` populates.
        let (server, record) = match source {
            ListenerSource::Prebound(PreboundListener::Tcp(tcp)) => {
                let record = match tcp.local_addr() {
                    Ok(addr) => {
                        bound_addrs.push(addr);
                        Some(Listener::tcp(addr, secure))
                    }
                    Err(e) => {
                        log::warn!("could not read local_addr of a bound listener: {e}");
                        Some(Listener::new(ListenerKind::Other("tcp".into()), secure))
                    }
                };
                (ServerType::from_tcp(tcp), record)
            }
            #[cfg(unix)]
            ListenerSource::Prebound(PreboundListener::Unix(unix)) => {
                let path = unix
                    .local_addr()
                    .ok()
                    .and_then(|addr| addr.as_pathname().map(Path::to_path_buf));
                (
                    ServerType::from_unix(unix),
                    Some(Listener::unix(path, secure)),
                )
            }
            ListenerSource::Adopted(server) => (server, None),
        };
        server.init(info);
        let record = record.or_else(|| {
            let derived = primary_listener(info, secure);
            if let Some(addr) = derived.as_ref().and_then(Listener::socket_addr) {
                bound_addrs.push(addr);
            }
            derived
        });
        adopted.push(AdoptedListener {
            server,
            acceptor,
            record,
        });
    }
    (adopted, bound_addrs)
}

/// Adopt each pre-claimed UDP socket into the runtime via its `BoxedQuicConfig`. `info` gives quic
/// adapters the same affordance as `Config`'s single-listener path. Endpoint construction happens
/// before `init_shared` so `Handler::init` sees any state the adapter populates and so we know each
/// quic port for alt-svc resolution.
fn adopt_quic_listeners<ServerType: Server>(
    quic_listeners: Vec<(StdUdpSocket, BoxedQuicConfig<ServerType>)>,
    runtime: &ServerType::Runtime,
    info: &mut Info,
) -> Vec<AdoptedQuic> {
    let mut adopted_quic = Vec::with_capacity(quic_listeners.len());
    for (socket, boxed_quic) in quic_listeners {
        let local_addr = socket.local_addr().ok();
        match boxed_quic.bind(socket, runtime.clone(), info) {
            Ok(endpoint) => adopted_quic.push((endpoint, local_addr)),
            Err(e) => log::error!("failed to bind QUIC endpoint at {local_addr:?}: {e}"),
        }
    }
    adopted_quic
}

/// Assemble the public listener set — adopted TCP/TLS and Unix listeners, then reuseport, then
/// QUIC — install it on `info`, and resolve the alt-svc header values from the combined TCP/QUIC
/// port sets plus any explicit pairings. TLS-vs-plaintext is each listener's own acceptor.
fn populate_listener_set<ServerType: Server>(
    info: &mut Info,
    standard: &[AdoptedListener<ServerType>],
    reuseport_listeners: &[ReuseportListener<ServerType>],
    quic: &[AdoptedQuic],
    alt_svc_pairs: &[(u16, u16)],
) -> HashMap<u16, &'static str> {
    let listener_set: Listeners = standard
        .iter()
        .filter_map(|l| l.record.clone())
        .chain(
            reuseport_listeners
                .iter()
                .map(|(addr, _, acceptor)| Listener::tcp(*addr, acceptor.is_secure())),
        )
        .chain(quic.iter().filter_map(|(_, addr)| addr.map(Listener::quic)))
        .collect();
    info.insert_shared_state(listener_set);

    let listener_ports: Vec<u16> = standard
        .iter()
        .filter_map(|l| l.record.as_ref().and_then(Listener::port))
        .chain(reuseport_listeners.iter().map(|(addr, _, _)| addr.port()))
        .collect();
    let quic_ports: Vec<u16> = quic
        .iter()
        .filter_map(|(_, addr)| addr.as_ref().map(SocketAddr::port))
        .collect();

    build_alt_svc_map(&listener_ports, &quic_ports, alt_svc_pairs)
}

/// The initialized state shared by every accept loop: the one `HttpContext` and handler produced by
/// `init_shared`, the runtime they run on, the connection-tuning knobs, and the resolved alt-svc
/// map. Cloned cheaply across the shared-runtime accept loops, the h3 loops, and — on platforms
/// that support it — the reuseport worker fleet.
pub(super) struct Shared<ServerType: Server, H: Handler> {
    pub(super) context: Arc<HttpContext>,
    pub(super) handler: ArcHandler<H>,
    pub(super) runtime: ServerType::Runtime,
    pub(super) max_connections: Option<usize>,
    pub(super) nodelay: bool,
    pub(super) alt_svc: HashMap<u16, &'static str>,
}

// Manual `Clone` so `ServerType`/`H` need not themselves be `Clone` (every field already is).
impl<ServerType: Server, H: Handler> Clone for Shared<ServerType, H> {
    fn clone(&self) -> Self {
        Self {
            context: self.context.clone(),
            handler: self.handler.clone(),
            runtime: self.runtime.clone(),
            max_connections: self.max_connections,
            nodelay: self.nodelay,
            alt_svc: self.alt_svc.clone(),
        }
    }
}

impl<ServerType: Server, H: Handler> Shared<ServerType, H> {
    /// Build the [`RunningConfig`] for one accept loop on `runtime` — sharing this context +
    /// handler and resolving the listener's `alt-svc` header value from its port — and return the
    /// future that drives it until shutdown.
    pub(super) fn serve(
        &self,
        acceptor: BoxedAcceptor<ServerType::Transport>,
        listener: Option<Listener>,
        runtime: ServerType::Runtime,
        server: ServerType,
    ) -> impl Future<Output = ()> + Send + 'static {
        let local_alt_svc = listener
            .as_ref()
            .and_then(Listener::port)
            .and_then(|port| self.alt_svc.get(&port).copied());
        let config = Arc::new(RunningConfig {
            acceptor,
            max_connections: self.max_connections,
            nodelay: self.nodelay,
            runtime,
            context: self.context.clone(),
            listener,
            local_alt_svc,
        });
        let handler = self.handler.clone();
        async move { config.run_async(server, handler).await }
    }

    /// Spawn every accept loop — h3 and TCP/Unix — detached, then hold this future open until the
    /// swansong shuts down. Each accept loop holds a swansong guard until it has finished cleaning
    /// up, so awaiting the swansong waits for the full graceful drain — no task join handles
    /// needed.
    pub(super) async fn drive(
        &self,
        standard: Vec<AdoptedListener<ServerType>>,
        quic: Vec<AdoptedQuic>,
        has_reuseport_workers: bool,
    ) {
        if standard.is_empty() && quic.is_empty() && !has_reuseport_workers {
            log::warn!("server spawned with no listeners; awaiting shutdown");
        }

        for (endpoint, local_addr) in quic {
            self.spawn_h3_loop(endpoint, local_addr);
        }
        for adopted in standard {
            self.spawn_tcp_loop(adopted);
        }

        self.context.swansong().clone().await;
    }

    /// Spawn an h3 accept loop for one QUIC endpoint, detached.
    fn spawn_h3_loop(&self, endpoint: ArcedQuicEndpoint, local_addr: Option<SocketAddr>) {
        let local_alt_svc = local_addr.and_then(|a| self.alt_svc.get(&a.port()).copied());
        let listener = local_addr.map(Listener::quic);
        let context = self.context.clone();
        let handler = self.handler.clone();
        let runtime = self.runtime.clone();
        self.runtime.spawn(async move {
            crate::h3::run_h3(endpoint, context, handler, runtime, listener, local_alt_svc).await;
        });
    }

    /// Spawn a TCP/Unix accept loop onto the shared runtime, detached.
    fn spawn_tcp_loop(&self, adopted: AdoptedListener<ServerType>) {
        self.runtime.spawn(self.serve(
            adopted.acceptor,
            adopted.record,
            self.runtime.clone(),
            adopted.server,
        ));
    }
}

/// On platforms without working `SO_REUSEPORT` load-balancing the `reuseport` cfg is off, so the
/// real fleet machinery in `reuseport.rs` is absent; nothing can have registered a fleet, so this
/// stub always reports none.
#[cfg(not(reuseport))]
impl<ServerType: Server, H: Handler> Shared<ServerType, H> {
    pub(super) fn spawn_reuseport_fleet_if_configured(
        &self,
        thread_per_core: Option<ThreadPerCoreInvoker<ServerType::Runtime>>,
        reuseport_listeners: Vec<ReuseportListener<ServerType>>,
        reuseport_workers: Option<usize>,
    ) -> bool {
        let _ = (thread_per_core, reuseport_listeners, reuseport_workers);
        false
    }
}
