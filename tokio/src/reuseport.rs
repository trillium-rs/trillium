use crate::{TokioRuntime, server::TokioServer};
use std::{
    fmt::{self, Debug, Formatter},
    net::{SocketAddr, ToSocketAddrs},
    num::NonZero,
    ops::Deref,
    thread::{self, JoinHandle},
};
use tokio::runtime::{Builder, Runtime};
use trillium::Handler;
use trillium_server_common::{Acceptor, Config, QuicConfig, Server, ServerHandle, bind_reuse_port};

/// A handle to a running [`ReuseportConfigExt::spawn_reuseport`] server.
///
/// Owns the shared multi-threaded runtime and the per-core worker threads, so dropping it tears
/// the server down. Derefs to [`ServerHandle`] for shutdown ([`ServerHandle::shut_down`]) and
/// bound-server introspection ([`ServerHandle::info`]).
pub struct ReuseportHandle {
    handle: ServerHandle,
    local_addr: SocketAddr,
    shared_runtime: Runtime,
    workers: Vec<JoinHandle<()>>,
}

impl Debug for ReuseportHandle {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReuseportHandle")
            .field("local_addr", &self.local_addr)
            .field("workers", &self.workers.len())
            .finish_non_exhaustive()
    }
}

impl Deref for ReuseportHandle {
    type Target = ServerHandle;

    fn deref(&self) -> &ServerHandle {
        &self.handle
    }
}

impl ReuseportHandle {
    /// The address the reuseport listener group is bound to (resolved before any worker started,
    /// so this reflects the kernel-assigned port even when binding to `:0`).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Park the current thread until the server shuts down and every worker has drained. This is
    /// what [`ReuseportConfigExt::run_reuseport`] does after spawning.
    pub fn block(self) {
        for worker in self.workers {
            let _ = worker.join();
        }
        drop(self.shared_runtime);
    }
}

/// Extends [`Config`] with a SO_REUSEPORT thread-per-core entrypoint.
///
/// Available only on Unix with the `reuseport` feature enabled.
pub trait ReuseportConfigExt {
    /// Spawn the handler across a fleet of single-threaded per-core runtimes behind a single
    /// SO_REUSEPORT listener group, plus one shared multi-threaded runtime, returning a
    /// [`ReuseportHandle`] without blocking.
    ///
    /// This is an alternative to [`Config::spawn`] for high-throughput deployments. Instead of one
    /// work-stealing runtime, it binds one `SO_REUSEPORT` TCP listener per worker so the kernel
    /// load-balances inbound connections across them, and runs each listener's accept loop on its
    /// own single-threaded runtime — keeping each connection's work on the core the kernel
    /// delivered it to, with no cross-core migration. [`Handler::init`] runs exactly once and the
    /// resulting handler (and all shared state) is shared across every worker.
    ///
    /// A separate multi-threaded runtime is always created. It owns the HTTP/3 endpoint (when QUIC
    /// is configured, since a single UDP endpoint cannot be sharded) and is the runtime exposed to
    /// the application as [`Runtime`](trillium_server_common::Runtime) in conn state — so handler
    /// `spawn`s land on a work-stealing pool with the usual invariants rather than pinning a core.
    ///
    /// Worker counts come from the `WORKERS` (per-core TCP listeners; defaults to the available
    /// parallelism) and `QUIC_THREADS` (shared multi-threaded runtime; defaults to a fraction of
    /// `WORKERS`) environment variables.
    fn spawn_reuseport(self, handler: impl Handler) -> ReuseportHandle;

    /// Equivalent to [`spawn_reuseport`](Self::spawn_reuseport) followed by
    /// [`ReuseportHandle::block`] — spawns the fleet and parks the calling thread until shutdown.
    fn run_reuseport(self, handler: impl Handler)
    where
        Self: Sized,
    {
        self.spawn_reuseport(handler).block();
    }
}

impl<AcceptorType, QuicType> ReuseportConfigExt for Config<TokioServer, AcceptorType, QuicType>
where
    AcceptorType: Acceptor<<TokioServer as Server>::Transport>,
    QuicType: QuicConfig<TokioServer>,
{
    fn spawn_reuseport(self, handler: impl Handler) -> ReuseportHandle {
        let host = self
            .host()
            .map(str::to_owned)
            .or_else(|| std::env::var("HOST").ok())
            .unwrap_or_else(|| "localhost".into());
        let port = self
            .port()
            .or_else(|| {
                std::env::var("PORT")
                    .ok()
                    .map(|p| p.parse().expect("PORT must be an unsigned integer"))
            })
            .unwrap_or(8080);

        let tcp_workers = worker_count("WORKERS", available_parallelism);
        let quic_workers = worker_count("QUIC_THREADS", || (tcp_workers / 4).clamp(2, 8));

        let bind_addr = (host.as_str(), port)
            .to_socket_addrs()
            .expect("could not resolve host and port")
            .next()
            .expect("host and port resolved to no addresses");

        // The shared multi-threaded runtime: owns the QUIC endpoint and signal handler, hosts
        // application spawns, and is the runtime that one-time initialization runs on.
        let shared_runtime = Builder::new_multi_thread()
            .worker_threads(quic_workers)
            .enable_all()
            .build()
            .expect("could not build multi-threaded runtime");

        // `config()` eagerly builds a runtime into the `Config` (the `TokioRuntime::default()`
        // we never use here). It would otherwise be dropped inside `initialize` — i.e. inside the
        // `block_on` below — and tokio panics when a runtime is dropped from an async context.
        // Hold a clone across the `block_on` so that drop is not the last one, then release it on
        // this (synchronous) thread.
        let eager_runtime = self.runtime();

        let (shared, handle, resolved, claiming) = shared_runtime.block_on(async move {
            let runtime = TokioRuntime::default();
            let claiming = bind_reuse_port(bind_addr).expect("could not bind reuseport listener");
            let resolved = claiming
                .local_addr()
                .expect("could not read reuseport listener address");

            let (shared, quic_binding, handle) =
                self.initialize(runtime.clone(), resolved, handler).await;

            shared.spawn_signals(runtime.clone());

            if let Some(endpoint) = quic_binding {
                let shared = shared.clone();
                let runtime = runtime.clone();
                runtime
                    .clone()
                    .spawn(async move { shared.h3_accept_loop(runtime, endpoint).await });
            }

            (shared, handle, resolved, claiming)
        });

        drop(eager_runtime);

        log::info!(
            "reuseport: {tcp_workers} per-core worker(s) on {resolved}, {quic_workers} shared \
             runtime thread(s)"
        );

        // Worker 0 adopts the listener already bound above (which holds the resolved port from the
        // moment of binding, so it is never released); the rest bind their own into the group.
        let mut claiming = Some(claiming);
        let workers = (0..tcp_workers)
            .map(|idx| {
                let shared = shared.clone();
                let claimed = claiming.take();
                thread::Builder::new()
                    .name(format!("trillium-reuseport-{idx}"))
                    .spawn(move || {
                        let runtime = Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("could not build current-thread runtime");
                        runtime.block_on(async move {
                            let worker_runtime = TokioRuntime::default();
                            let listener = claimed.unwrap_or_else(|| {
                                bind_reuse_port(resolved)
                                    .expect("could not bind reuseport listener")
                            });
                            shared
                                .accept_loop(worker_runtime, TokioServer::from_tcp(listener))
                                .await;
                        });
                    })
                    .expect("could not spawn worker thread")
            })
            .collect::<Vec<_>>();

        ReuseportHandle {
            handle,
            local_addr: resolved,
            shared_runtime,
            workers,
        }
    }
}

fn worker_count(env_key: &str, default: impl FnOnce() -> usize) -> usize {
    std::env::var(env_key)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(default)
        .max(1)
}

fn available_parallelism() -> usize {
    thread::available_parallelism().map_or(1, NonZero::get)
}
