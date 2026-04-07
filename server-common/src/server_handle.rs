use crate::Runtime;
use async_cell::sync::AsyncCell;
use std::{cell::OnceCell, future::IntoFuture, net::SocketAddr, sync::Arc};
use swansong::{ShutdownCompletion, Swansong};
use trillium_http::HttpContext;

/// A handle for a spawned trillium server. Returned by
/// [`Config::handle`][crate::Config::handle] and
/// [`Config::spawn`][crate::Config::spawn]
#[derive(Clone, Debug)]
pub struct ServerHandle {
    pub(crate) swansong: Swansong,
    pub(crate) context: Arc<AsyncCell<Arc<HttpContext>>>,
    pub(crate) received_context: OnceCell<Arc<HttpContext>>,
    pub(crate) runtime: Runtime,
}

/// Immutable snapshot of server state after initialization.
///
/// Returned by [`ServerHandle::info`]. Contains the bound address, derived URL, and any
/// values inserted into shared state during [`Handler::init`](trillium::Handler::init).
#[derive(Debug)]
pub struct BoundInfo(Arc<HttpContext>);

impl BoundInfo {
    /// Borrow a type from the [`TypeSet`](trillium::TypeSet) on this `BoundInfo`.
    pub fn shared_state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.0.shared_state().get()
    }

    /// Returns the `local_addr` of a bound tcp listener, if such a thing exists for this server
    pub fn tcp_socket_addr(&self) -> Option<&SocketAddr> {
        self.shared_state()
    }

    /// Returns the URL of this server, derived from the bound address, if available
    pub fn url(&self) -> Option<&url::Url> {
        self.shared_state()
    }

    /// Returns the `local_addr` of a bound unix listener, if such a thing exists for this server
    #[cfg(unix)]
    pub fn unix_socket_addr(&self) -> Option<&std::os::unix::net::SocketAddr> {
        self.shared_state()
    }

    /// Returns a clone of the underlying [`HttpContext`] for this server
    pub fn context(&self) -> Arc<HttpContext> {
        self.0.clone()
    }
}

impl ServerHandle {
    /// await server start and retrieve the server's [`Info`](trillium::Info)
    pub async fn info(&self) -> BoundInfo {
        if let Some(context) = self.received_context.get().cloned() {
            return BoundInfo(context);
        }
        let arc_context = self.context.get().await;
        let context = self.received_context.get_or_init(|| arc_context);

        BoundInfo(Arc::clone(context))
    }

    /// stop server and return a future that can be awaited for it to shut down gracefully
    pub fn shut_down(&self) -> ShutdownCompletion {
        self.swansong.shut_down()
    }

    /// retrieves a clone of the [`Swansong`] used by this server
    pub fn swansong(&self) -> Swansong {
        self.swansong.clone()
    }

    /// retrieves a runtime
    pub fn runtime(&self) -> Runtime {
        self.runtime.clone()
    }
}

impl IntoFuture for ServerHandle {
    type IntoFuture = ShutdownCompletion;
    type Output = ();

    fn into_future(self) -> Self::IntoFuture {
        self.swansong.into_future()
    }
}
