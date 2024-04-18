use crate::Runtime;
use async_cell::sync::AsyncCell;
use std::{cell::OnceCell, future::IntoFuture, sync::Arc};
use swansong::{ShutdownCompletion, Swansong};
use trillium::Info;

/// A handle for a spawned trillium server. Returned by
/// [`Config::handle`][crate::Config::handle] and
/// [`Config::spawn`][crate::Config::spawn]
#[derive(Clone, Debug)]
pub struct ServerHandle {
    pub(crate) swansong: Swansong,
    pub(crate) info: Arc<AsyncCell<Arc<Info>>>,
    pub(crate) received_info: OnceCell<Arc<Info>>,
    pub(crate) runtime: Runtime,
}

impl ServerHandle {
    /// await server start and retrieve the server's [`Info`]
    pub async fn info(&self) -> &Info {
        if let Some(info) = self.received_info.get() {
            return info;
        }
        let arc_info = self.info.get().await;
        self.received_info.get_or_init(|| arc_info)
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
    type Output = ();

    type IntoFuture = ShutdownCompletion;

    fn into_future(self) -> Self::IntoFuture {
        self.swansong.into_future()
    }
}
