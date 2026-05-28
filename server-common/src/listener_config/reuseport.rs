//! `SO_REUSEPORT` thread-per-core binding, gated behind the `reuseport` cfg. Holds the
//! [`FanOut`](crate::FanOut)-bounded bind methods on [`ListenerConfig`] plus the worker-fleet
//! machinery that [`ListenerConfig::run_async`](super::ListenerConfig::run_async) spawns.

use super::{
    BoxedWorker, IntoListenAddr, ListenerConfig, ReuseportListener, ThreadPerCoreInvoker,
    run::Shared,
};
use crate::{Acceptor, BoxedAcceptor, FanOut, RuntimeTrait, Server};
use std::{
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    thread::JoinHandle,
};
use trillium::{Handler, Listener};

/// Reuseport thread-per-core binds, available only on platforms where `SO_REUSEPORT` actually
/// load-balances connections (Linux and other non-Apple, non-Solaris/illumos/cygwin Unixes) and
/// only for runtimes whose [`Runtime`](crate::Runtime) implements [`FanOut`](crate::FanOut). The
/// trait bound is the gate: a runtime without a `FanOut` impl simply does not have these methods.
impl<ServerType: Server> ListenerConfig<ServerType>
where
    ServerType::Runtime: FanOut,
{
    /// Register a plaintext TCP listener fanned across per-core worker threads.
    ///
    /// Unlike [`bind_tcp`](Self::bind_tcp), which adopts one listener onto the shared
    /// multi-threaded runtime, this binds one `SO_REUSEPORT` listener per worker thread into a
    /// kernel reuseport group and runs each listener's accept loop on its own single-threaded
    /// executor — keeping each connection's work on the core the kernel delivered it to. The
    /// address is claimed immediately (resolving `:0` to the group's shared port), failing fast.
    pub fn bind_reuseport_tcp(self, addr: impl IntoListenAddr) -> std::io::Result<Self> {
        self.bind_reuseport(addr.into_listen_addr()?, BoxedAcceptor::new(()))
    }

    /// Register a TLS listener fanned across per-core worker threads. The reuseport equivalent of
    /// [`bind_tls`](Self::bind_tls); see [`bind_reuseport_tcp`](Self::bind_reuseport_tcp) for the
    /// fan-out semantics.
    pub fn bind_reuseport_tls<A>(
        self,
        addr: impl IntoListenAddr,
        acceptor: A,
    ) -> std::io::Result<Self>
    where
        A: Acceptor<ServerType::Transport>,
    {
        self.bind_reuseport(addr.into_listen_addr()?, BoxedAcceptor::new(acceptor))
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
    ) -> std::io::Result<Self> {
        let listener = crate::bind_reuse_port(addr)?;
        let resolved = listener.local_addr()?;
        self.reuseport_listeners
            .push((resolved, listener, acceptor));
        self.thread_per_core = Some(invoke_thread_per_core::<ServerType::Runtime>);
        Ok(self)
    }
}

/// Invoke [`FanOut::thread_per_core`](crate::FanOut::thread_per_core) on a concrete runtime with a
/// type-erased worker. Coerced to a [`ThreadPerCoreInvoker`] function pointer at the
/// `bind_reuseport_*` call site, where the `FanOut` bound is available.
fn invoke_thread_per_core<R: FanOut>(
    runtime: &R,
    count: usize,
    worker: BoxedWorker,
) -> Vec<JoinHandle<()>> {
    runtime.thread_per_core(count, move |idx| worker(idx))
}

impl<ServerType: Server, H: Handler> Shared<ServerType, H> {
    /// Spawn the reuseport worker fleet if a `bind_reuseport_*` method installed a thread-per-core
    /// invoker, returning whether any workers were spawned. The join handles are detached — workers
    /// drain via the shared [`Swansong`](crate::Swansong).
    pub(super) fn spawn_reuseport_fleet_if_configured(
        &self,
        thread_per_core: Option<ThreadPerCoreInvoker<ServerType::Runtime>>,
        reuseport_listeners: Vec<ReuseportListener<ServerType>>,
        reuseport_workers: Option<usize>,
    ) -> bool {
        let Some(invoke) = thread_per_core else {
            return false;
        };
        let _workers = self.spawn_reuseport_fleet(invoke, reuseport_listeners, reuseport_workers);
        true
    }

    /// Spawn the per-core worker fleet. Each worker runs one accept loop per reuseport bind on its
    /// own single-threaded executor; worker 0 adopts the listener claimed eagerly at bind time, and
    /// the rest bind fresh members into the kernel reuseport group.
    fn spawn_reuseport_fleet(
        &self,
        invoke: ThreadPerCoreInvoker<ServerType::Runtime>,
        listeners: Vec<ReuseportListener<ServerType>>,
        workers: Option<usize>,
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
                (addr, Arc::new(Mutex::new(Some(listener))), acceptor)
            })
            .collect();
        let binds = Arc::new(binds);

        log::info!(
            "reuseport: {count} per-core worker(s) across {} listener(s)",
            binds.len()
        );

        let shared = self.clone();
        let worker: BoxedWorker = Arc::new(move |idx: usize| {
            let binds = Arc::clone(&binds);
            let shared = shared.clone();
            Box::pin(async move {
                let runtime = ServerType::runtime();
                for (addr, claimed, acceptor) in binds.iter() {
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
                    let server = ServerType::from_tcp(listener);
                    runtime.spawn(shared.serve(
                        acceptor.clone(),
                        Some(Listener::tcp(*addr, acceptor.is_secure())),
                        runtime.clone(),
                        server,
                    ));
                }
                // Each accept loop holds a swansong guard until it has cleaned up, so awaiting the
                // swansong keeps this worker's executor alive for the full graceful drain.
                shared.context.swansong().clone().await;
            }) as Pin<Box<dyn Future<Output = ()>>>
        });

        invoke(&self.runtime, count, worker)
    }
}

fn available_parallelism() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
}
