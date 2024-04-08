use crate::Runtime;
use async_cell::sync::AsyncCell;
use std::{cell::OnceCell, future::IntoFuture, net::SocketAddr, sync::Arc};
use swansong::{ShutdownCompletion, Swansong};
use trillium_http::ServerConfig;

/// A handle for a spawned trillium server. Returned by
/// [`Config::handle`][crate::Config::handle] and
/// [`Config::spawn`][crate::Config::spawn]
#[derive(Clone, Debug)]
pub struct ServerHandle {
    pub(crate) swansong: Swansong,
    pub(crate) server_config: Arc<AsyncCell<Arc<ServerConfig>>>,
    pub(crate) received_server_config: OnceCell<Arc<ServerConfig>>,
    pub(crate) runtime: Runtime,
}

#[derive(Debug)]
pub struct BoundInfo(Arc<ServerConfig>);

impl BoundInfo {
    /// Borrow a type from the [`TypeSet`] on this `BoundInfo`.
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.0.shared_state().get()
    }

    /// Returns the `local_addr` of a bound tcp listener, if such a thing exists for this server
    pub fn tcp_socket_addr(&self) -> Option<&SocketAddr> {
        self.state()
    }

    pub fn url(&self) -> Option<&url::Url> {
        self.state()
    }

    /// Returns the `local_addr` of a bound unix listener, if such a thing exists for this server
    #[cfg(unix)]
    pub fn unix_socket_addr(&self) -> Option<&std::os::unix::net::SocketAddr> {
        self.state()
    }
}

impl ServerHandle {
    /// await server start and retrieve the server's [`Info`]
    pub async fn info(&self) -> BoundInfo {
        if let Some(server_config) = self.received_server_config.get().cloned() {
            return BoundInfo(server_config);
        }
        let arc_server_config = self.server_config.get().await;
        let server_config = self
            .received_server_config
            .get_or_init(|| arc_server_config);

        BoundInfo(Arc::clone(server_config))
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
