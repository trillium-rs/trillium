use crate::{Conn, ConnectionStatus, HttpConfig, Result, TypeSet, Upgrade};
use futures_lite::{AsyncRead, AsyncWrite};
use std::{future::Future, sync::Arc};
use swansong::{ShutdownCompletion, Swansong};
/// This struct represents the shared configuration and context for a http server.
///
/// This currently contains tunable parameters in a [`HttpConfig`], the [`Swansong`] graceful
/// shutdown control interface, and a shared [`TypeSet`] that contains application-specific
/// information about the running server
#[derive(Default, Debug)]
pub struct ServerConfig {
    pub(crate) http_config: HttpConfig,
    pub(crate) swansong: Swansong,
    pub(crate) shared_state: TypeSet,
}
impl AsRef<TypeSet> for ServerConfig {
    fn as_ref(&self) -> &TypeSet {
        &self.shared_state
    }
}

impl AsMut<TypeSet> for ServerConfig {
    fn as_mut(&mut self) -> &mut TypeSet {
        &mut self.shared_state
    }
}

impl AsRef<Swansong> for ServerConfig {
    fn as_ref(&self) -> &Swansong {
        &self.swansong
    }
}

impl AsRef<HttpConfig> for ServerConfig {
    fn as_ref(&self) -> &HttpConfig {
        &self.http_config
    }
}

impl ServerConfig {
    /// Modify the [`HttpConfig`] for this server.
    pub fn http_config_mut(&mut self) -> &mut HttpConfig {
        &mut self.http_config
    }

    /// Replace the [`Swansong`] graceful shutdown control interface for this server.
    pub fn set_swansong(&mut self, swansong: Swansong) {
        self.swansong = swansong;
    }

    /// Borrow the [`Swansong`] graceful shutdown control interface for this server.
    pub fn swansong(&self) -> &Swansong {
        &self.swansong
    }

    /// Construct a new `ServerConfig`
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrow the shared state [`TypeSet`] for this server
    pub fn shared_state(&self) -> &TypeSet {
        &self.shared_state
    }

    /// Mutate the shared state [`TypeSet`] for this server.
    ///
    /// Types added here will be immutably available on all [`Conn`]s handled by this server.
    pub fn shared_state_mut(&mut self) -> &mut TypeSet {
        &mut self.shared_state
    }

    /// Perform HTTP on the provided transport, applying the provided `async Conn -> Conn` handler
    /// function for every distinct http request-response.
    ///
    /// For any given invocation of `ServerConfig::run`, the handler function may run any number of
    /// times, depending on whether the connection is reused by the client.
    ///
    /// This can only be called on an `Arc<ServerConfig>` because an arc clone is moved into the
    /// Conn.
    ///
    /// # Errors
    ///
    /// This function will return an [`Error`] if any of the http requests is irrecoverably
    /// malformed or otherwise noncompliant.
    pub async fn run<Transport, Handler, Fut>(
        self: Arc<Self>,
        transport: Transport,
        mut handler: Handler,
    ) -> Result<Option<Upgrade<Transport>>>
    where
        Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
        Handler: FnMut(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>>,
    {
        let _guard = self.swansong.guard();
        let buffer = Vec::with_capacity(self.http_config.request_buffer_initial_len).into();

        let mut conn = Conn::new_internal(self, transport, buffer).await?;

        loop {
            conn = match handler(conn).await.send().await? {
                ConnectionStatus::Upgrade(upgrade) => return Ok(Some(upgrade)),
                ConnectionStatus::Close => return Ok(None),
                ConnectionStatus::Conn(next) => next,
            }
        }
    }

    /// Attempt graceful shutdown of this server.
    ///
    /// The returned [`ShutdownCompletion`] type can
    /// either be awaited in an async context or blocked on with [`ShutdownCompletion::block`] in a
    /// blocking context
    pub fn shut_down(&self) -> ShutdownCompletion {
        self.swansong.shut_down()
    }
}
