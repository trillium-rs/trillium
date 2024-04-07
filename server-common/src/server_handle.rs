use async_cell::sync::AsyncCell;
use std::{future::IntoFuture, sync::Arc};
use swansong::{ShutdownCompletion, Swansong};
use trillium::Info;

/// A handle for a spawned trillium server. Returned by
/// [`Config::handle`][crate::Config::handle] and
/// [`Config::spawn`][crate::Config::spawn]
#[derive(Clone, Debug)]
pub struct ServerHandle {
    pub(crate) swansong: Swansong,
    pub(crate) info: Arc<AsyncCell<Arc<Info>>>,
}

impl ServerHandle {
    /// await server start and retrieve the server's [`Info`]
    pub async fn info(&self) -> Arc<Info> {
        self.info.get().await
    }

    /// stop server and wait for it to shut down gracefully
    pub async fn shut_down(&self) {
        self.swansong.shut_down().await;
    }

    /// retrieves a clone of the [`Swansong`] used by this server
    pub fn swansong(&self) -> Swansong {
        self.swansong.clone()
    }
}

impl IntoFuture for ServerHandle {
    type Output = ();

    type IntoFuture = ShutdownCompletion;

    fn into_future(self) -> Self::IntoFuture {
        self.swansong.into_future()
    }
}
