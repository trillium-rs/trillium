use crate::{
    Acceptor, ArcHandler, QuicEndpoint, RuntimeTrait, Server, running_config::RunningConfig,
};
use futures_lite::StreamExt;
use std::{
    fmt::{self, Debug, Formatter},
    marker::PhantomData,
    pin::pin,
    sync::Arc,
};
use trillium::Handler;
use trillium_http::HttpContext;

/// A server that has completed one-time initialization and can be run across any number of
/// runtimes and listeners.
///
/// This is produced after [`Handler::init`] has run exactly once, holding the resulting
/// `Arc`-shared handler and [`HttpContext`]. Cloning it is cheap and shares that single
/// initialized handler, so the same [`SharedServer`] can drive many accept loops — for example
/// one per-core listener under a SO_REUSEPORT fan-out — without re-initializing the handler or
/// duplicating shared state.
pub struct SharedServer<ServerType, AcceptorType, H>
where
    ServerType: Server,
    H: Handler,
{
    acceptor: AcceptorType,
    max_connections: Option<usize>,
    nodelay: bool,
    register_signals: bool,
    context: Arc<HttpContext>,
    handler: ArcHandler<H>,
    _server: PhantomData<fn() -> ServerType>,
}

impl<ServerType: Server, AcceptorType: Clone, H: Handler> Clone
    for SharedServer<ServerType, AcceptorType, H>
{
    fn clone(&self) -> Self {
        Self {
            acceptor: self.acceptor.clone(),
            max_connections: self.max_connections,
            nodelay: self.nodelay,
            register_signals: self.register_signals,
            context: self.context.clone(),
            handler: self.handler.clone(),
            _server: PhantomData,
        }
    }
}

impl<ServerType: Server, AcceptorType, H: Handler> Debug
    for SharedServer<ServerType, AcceptorType, H>
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedServer")
            .field("max_connections", &self.max_connections)
            .field("nodelay", &self.nodelay)
            .field("register_signals", &self.register_signals)
            .finish_non_exhaustive()
    }
}

impl<ServerType, AcceptorType, H> SharedServer<ServerType, AcceptorType, H>
where
    ServerType: Server,
    AcceptorType: Acceptor<ServerType::Transport>,
    H: Handler,
{
    pub(crate) fn new(
        acceptor: AcceptorType,
        max_connections: Option<usize>,
        nodelay: bool,
        register_signals: bool,
        context: Arc<HttpContext>,
        handler: H,
    ) -> Self {
        Self {
            acceptor,
            max_connections,
            nodelay,
            register_signals,
            context,
            handler: ArcHandler::new(handler),
            _server: PhantomData,
        }
    }

    pub(crate) fn context(&self) -> &Arc<HttpContext> {
        &self.context
    }

    /// Accept connections from `listener` on `runtime` until the shared
    /// [`Swansong`](crate::Swansong) shuts down. Each call drives an independent accept loop
    /// against the one shared handler.
    #[doc(hidden)]
    pub async fn accept_loop(&self, runtime: ServerType::Runtime, listener: ServerType) {
        Arc::new(RunningConfig {
            acceptor: self.acceptor.clone(),
            max_connections: self.max_connections,
            context: self.context.clone(),
            runtime,
            nodelay: self.nodelay,
        })
        .run_async(listener, self.handler.clone())
        .await;
    }

    /// Drive the HTTP/3 accept loop for a bound QUIC `endpoint` on `runtime`, dispatching to the
    /// shared handler.
    #[doc(hidden)]
    pub async fn h3_accept_loop(&self, runtime: ServerType::Runtime, endpoint: impl QuicEndpoint) {
        crate::h3::run_h3(
            endpoint,
            self.context.clone(),
            self.handler.clone(),
            runtime,
        )
        .await;
    }

    /// Spawn the OS-signal graceful-shutdown handler onto `runtime`, if signal handling is enabled.
    /// Intended to be called once for the whole server, regardless of how many accept loops run.
    #[doc(hidden)]
    pub fn spawn_signals(&self, runtime: ServerType::Runtime) {
        if !self.register_signals {
            return;
        }

        let swansong = self.context.swansong().clone();
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
}
