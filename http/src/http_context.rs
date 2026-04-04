use crate::{Conn, ConnectionStatus, HttpConfig, Result, TypeSet, Upgrade};
use fieldwork::Fieldwork;
use futures_lite::{AsyncRead, AsyncWrite};
use std::{future::Future, sync::Arc};
use swansong::{ShutdownCompletion, Swansong};
/// This struct represents the shared configuration and context for a http server.
///
/// This currently contains tunable parameters in a [`HttpConfig`], the [`Swansong`] graceful
/// shutdown control interface, and a shared [`TypeSet`] that contains application-specific
/// information about the running server
#[derive(Default, Debug, Fieldwork)]
#[fieldwork(get, set, get_mut, with)]
pub struct HttpContext {
    /// [`HttpConfig`] performance and security parameters
    pub(crate) config: HttpConfig,

    /// [`Swansong`] graceful shutdown interface
    pub(crate) swansong: Swansong,

    /// [`TypeSet`] shared state
    pub(crate) shared_state: TypeSet,
}
impl AsRef<TypeSet> for HttpContext {
    fn as_ref(&self) -> &TypeSet {
        &self.shared_state
    }
}

impl AsMut<TypeSet> for HttpContext {
    fn as_mut(&mut self) -> &mut TypeSet {
        &mut self.shared_state
    }
}

impl AsRef<Swansong> for HttpContext {
    fn as_ref(&self) -> &Swansong {
        &self.swansong
    }
}

impl AsRef<HttpConfig> for HttpContext {
    fn as_ref(&self) -> &HttpConfig {
        &self.config
    }
}

impl HttpContext {
    /// Construct a new `HttpContext`
    pub fn new() -> Self {
        Self::default()
    }

    /// Perform HTTP on the provided transport, applying the provided `async Conn -> Conn` handler
    /// function for every distinct http request-response.
    ///
    /// For any given invocation of `HttpContext::run`, the handler function may run any number of
    /// times, depending on whether the connection is reused by the client.
    ///
    /// This can only be called on an `Arc<HttpContext>` because an arc clone is moved into the
    /// Conn.
    ///
    /// # Errors
    ///
    /// This function will return an [`Error`](crate::Error) if any of the http requests is
    /// irrecoverably malformed or otherwise noncompliant.
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
        let buffer = Vec::with_capacity(self.config.request_buffer_initial_len).into();

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
