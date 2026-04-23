use crate::{
    Buffer, Conn, ConnectionStatus, HttpConfig, Result, TypeSet, Upgrade,
    headers::qpack::HeaderObserver,
};
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

    /// Per-listener QPACK header-frequency observer. Shared by `Arc` across all connections
    /// a given listener accepts; runtime adapters isolate it per hop-and-direction via
    /// [`__isolate_qpack_observer`](Self::__isolate_qpack_observer).
    #[cfg_attr(not(feature = "unstable"), field = false)]
    pub(crate) observer: Arc<HeaderObserver>,
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
        handler: Handler,
    ) -> Result<Option<Upgrade<Transport>>>
    where
        Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
        Handler: FnMut(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>>,
    {
        let initial_bytes = Vec::with_capacity(self.config.request_buffer_initial_len);
        run_with_initial_bytes(self, transport, initial_bytes, handler).await
    }

    /// Attempt graceful shutdown of this server.
    ///
    /// The returned [`ShutdownCompletion`] type can
    /// either be awaited in an async context or blocked on with [`ShutdownCompletion::block`] in a
    /// blocking context
    pub fn shut_down(&self) -> ShutdownCompletion {
        self.swansong.shut_down()
    }

    /// Replace this context's QPACK header observer with a fresh, empty one.
    ///
    /// Runtime adapters (trillium-server-common, trillium-client) call this during listener
    /// setup so that each hop-and-direction pair in a deployment gets its own observer. A
    /// reverse proxy's inbound server observer is distinct from its outbound client observer
    /// by construction, so header values one hop forwards (e.g. `authorization`, `cookie`)
    /// cannot reach the QPACK state of unrelated clients on the other hop.
    ///
    /// Not part of the stable public API; exposed only so adapter crates can call it.
    #[doc(hidden)]
    pub fn __isolate_qpack_observer(&mut self) -> &mut Self {
        self.observer = Arc::new(HeaderObserver::default());
        log::trace!(
            target: "qpack_metrics",
            "isolated fresh QPACK observer for this context (ptr={:p})",
            Arc::as_ptr(&self.observer),
        );
        self
    }
}

/// Like [`HttpContext::run`], but starts with the supplied bytes pre-filled into the request
/// buffer.
///
/// Used by runtime adapters that need to peek the first few bytes off a cleartext TCP stream
/// to decide between HTTP/1.1 and HTTP/2 prior-knowledge dispatch, and then hand those bytes
/// into the HTTP/1 parser without re-reading them.
///
/// The mechanism is the same one the keep-alive / pipelining path already uses when a follow-up
/// request arrives on the tail of the previous response: any bytes already in the buffer are
/// consumed by the parser before the next transport read.
///
/// # Errors
///
/// Same as [`HttpContext::run`] — any irrecoverably malformed or noncompliant HTTP/1 request
/// surfaces as an [`Error`](crate::Error).
pub async fn run_with_initial_bytes<Transport, Handler, Fut>(
    context: Arc<HttpContext>,
    transport: Transport,
    initial_bytes: Vec<u8>,
    mut handler: Handler,
) -> Result<Option<Upgrade<Transport>>>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    Handler: FnMut(Conn<Transport>) -> Fut,
    Fut: Future<Output = Conn<Transport>>,
{
    let _guard = context.swansong.guard();
    let buffer: Buffer = initial_bytes.into();
    let mut conn = Conn::new_internal(context, transport, buffer).await?;

    loop {
        conn = match handler(conn).await.send().await? {
            ConnectionStatus::Upgrade(upgrade) => return Ok(Some(upgrade)),
            ConnectionStatus::Close => return Ok(None),
            ConnectionStatus::Conn(next) => next,
        }
    }
}
