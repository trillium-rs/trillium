use crate::{Conn, ConnExt, Result};
use std::{
    any::Any,
    borrow::Cow,
    fmt::{self, Debug, Formatter},
    future::Future,
    pin::Pin,
    sync::Arc,
};

/// Client middleware extension point.
///
/// [`ClientHandler`] is the composition primitive for trillium-client middleware. It mirrors the
/// server-side [`trillium::Handler`] in spirit — handlers compose into tuples, halting on the conn
/// short-circuits the chain — but differs in shape because the client has different ownership
/// semantics: the conn is user-owned, so handlers take `&mut Conn` rather than owned `Conn`, and
/// they return `Result<()>` because client execution can fail outright (TLS handshake, refresh
/// token, signing, etc.).
///
/// [`trillium::Handler`]: https://docs.trillium.rs/trillium/trait.Handler.html
///
/// ## Lifecycle
///
/// Each `Conn::exec` call runs handlers in three steps:
///
/// 1. **Forward pass — `run`.** Each handler runs in declared order. A handler may mutate the
///    request, short-circuit by [calling `Conn::halt`] + populating synthetic response state (cache
///    hit, mocked response), or fail. If any handler halts, subsequent `run` methods are skipped.
/// 2. **Network round-trip.** Skipped if the conn is halted.
/// 3. **Reverse pass — `after_response`.** Each handler's `after_response` runs in *reverse* order,
///    *regardless of halt status*. This mirrors `trillium::Handler::before_send` and lets handlers
///    that observe the response record cache hits and short-circuited responses, not just
///    transport-backed ones.
///
/// [calling `Conn::halt`]: crate::Conn::halt
///
/// ## Re-execution
///
/// Handlers that need to re-issue a request (follow-redirects, retry, auth-refresh) build a fresh
/// `Conn` from `conn.client()` in `after_response`, configure it (filtered headers, replayed body,
/// handler-internal state), and queue it via
/// [`ConnExt::set_followup`][crate::ConnExt::set_followup]. The trampoline in
/// [`IntoFuture for &mut Conn`][std::future::IntoFuture] picks the follow-up up after the current
/// cycle's `after_response` has fully unwound: it recycles the current response body, swaps the
/// follow-up into place, and runs another full `(run → network → after_response)` cycle on it.
///
/// ## Handler-author affordances on `Conn`
///
/// Lifecycle-driving methods — queue a follow-up, stash or recover the transport-level error —
/// live on the [`ConnExt`][crate::ConnExt] extension trait rather than directly on
/// [`Conn`]. Bring them into scope with `use trillium_client::ConnExt;`. The split is
/// intentional: those operations are meaningful only from inside a handler, and keeping them off
/// `Conn`'s inherent surface stops them from appearing in IDE completion for user code that holds
/// a `Conn` directly.
///
/// ## Type erasure
///
/// Implementors write [`ClientHandler`] using native `async fn` syntax. The crate type-erases
/// handlers internally for storage on `Client`; [`Client::with_handler`] accepts any
/// `impl ClientHandler`, and [`Client::downcast_handler`] is the way to recover the concrete type
/// from a `Client` that has one installed.
///
/// [`Client::with_handler`]: crate::Client::with_handler
/// [`Client::downcast_handler`]: crate::Client::downcast_handler
pub trait ClientHandler: Send + Sync + 'static {
    /// Forward-pass hook, called before the network round-trip in declared order.
    ///
    /// A handler can mutate the request, halt to short-circuit, or fail. The default
    /// implementation is a no-op.
    fn run(&self, conn: &mut Conn) -> impl Future<Output = Result<()>> + Send {
        let _ = conn;
        async { Ok(()) }
    }

    /// Reverse-pass hook, called after the network round-trip (or after a halt-skipped network
    /// call) in *reverse* declared order. Always runs regardless of halt status or transport
    /// error.
    ///
    /// A handler can observe the response, mutate it before passing it to upstream handlers,
    /// recover from a transport-level error, or fail.
    ///
    /// **Transport errors.** If the network call failed (connect refused, TLS handshake error,
    /// malformed HTTP frame, timeout), the framework stashes the error on the conn and runs
    /// `after_response` anyway. A handler that recovers from an error should:
    /// 1. Inspect [`conn.error()`][crate::ConnExt::error] to detect the failure.
    /// 2. Populate response state synthetically (`set_status`, `response_headers_mut`,
    ///    `set_response_body`) or enqueue a new followup conn.
    /// 3. Call [`conn.take_error()`][crate::ConnExt::take_error] to clear the error so the awaited
    ///    conn returns `Ok`.
    ///
    /// The `error` / `take_error` / `set_error` methods live on the
    /// [`ConnExt`][crate::ConnExt] extension trait — `use
    /// trillium_client::ConnExt;` to bring them into scope.
    ///
    /// If no handler clears the error, it propagates as `Err` from the awaited conn.
    ///
    /// The default implementation is a no-op.
    fn after_response(&self, conn: &mut Conn) -> impl Future<Output = Result<()>> + Send {
        let _ = conn;
        async { Ok(()) }
    }

    /// Human-readable name for logging/debugging. Defaults to the type name.
    fn name(&self) -> Cow<'static, str> {
        std::any::type_name::<Self>().into()
    }
}

/// Object-safe twin of [`ClientHandler`] used for internal type erasure. Users implement
/// [`ClientHandler`] with native `async fn`; the blanket impl below adapts it.
pub(crate) trait ObjectSafeClientHandler: Any + Send + Sync + 'static {
    fn run<'a>(
        &'a self,
        conn: &'a mut Conn,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
    fn after_response<'a>(
        &'a self,
        conn: &'a mut Conn,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;
    fn name(&self) -> Cow<'static, str>;
    fn as_any(&self) -> &dyn Any;
}

impl<H: ClientHandler> ObjectSafeClientHandler for H {
    fn run<'a>(
        &'a self,
        conn: &'a mut Conn,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(ClientHandler::run(self, conn))
    }

    fn after_response<'a>(
        &'a self,
        conn: &'a mut Conn,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(ClientHandler::after_response(self, conn))
    }

    fn name(&self) -> Cow<'static, str> {
        ClientHandler::name(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Internal `Arc`-shared, type-erased [`ClientHandler`]. Stored on a `Client` and cloned onto
/// each conn it builds. Not exposed publicly — `Client::with_handler` accepts
/// `impl ClientHandler` and `Client::downcast_handler` recovers the concrete type.
#[derive(Clone)]
pub(crate) struct ArcedClientHandler(Arc<dyn ObjectSafeClientHandler>);

impl Debug for ArcedClientHandler {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ArcedClientHandler")
            .field(&self.0.name())
            .finish()
    }
}

impl ArcedClientHandler {
    pub(crate) fn new(handler: impl ClientHandler) -> Self {
        Self(Arc::new(handler))
    }

    pub(crate) fn downcast_ref<T: Any + 'static>(&self) -> Option<&T> {
        self.0.as_any().downcast_ref()
    }
}

impl ClientHandler for ArcedClientHandler {
    async fn run(&self, conn: &mut Conn) -> Result<()> {
        self.0.run(conn).await
    }

    async fn after_response(&self, conn: &mut Conn) -> Result<()> {
        self.0.after_response(conn).await
    }

    fn name(&self) -> Cow<'static, str> {
        self.0.name()
    }
}

impl ClientHandler for () {}

impl<H: ClientHandler> ClientHandler for Option<H> {
    async fn run(&self, conn: &mut Conn) -> Result<()> {
        if let Some(h) = self {
            h.run(conn).await?;
        }
        Ok(())
    }

    async fn after_response(&self, conn: &mut Conn) -> Result<()> {
        if let Some(h) = self {
            h.after_response(conn).await?;
        }
        Ok(())
    }

    fn name(&self) -> Cow<'static, str> {
        match self {
            Some(h) => h.name(),
            None => "None".into(),
        }
    }
}

macro_rules! reverse_after_response {
    ($conn:ident, $name:ident) => {
        log::trace!("after_response {}", $name.name());
        $name.after_response($conn).await?;
    };
    ($conn:ident, $name:ident $($rest:ident)+) => {
        reverse_after_response!($conn, $($rest)+);
        log::trace!("after_response {}", $name.name());
        $name.after_response($conn).await?;
    };
}

macro_rules! impl_client_handler_tuple {
    ($($name:ident)+) => {
        impl<$($name: ClientHandler),+> ClientHandler for ($($name,)+) {
            #[allow(non_snake_case)]
            async fn run(&self, conn: &mut Conn) -> Result<()> {
                let ($(ref $name,)+) = *self;
                $(
                    log::trace!("running {}", $name.name());
                    $name.run(conn).await?;
                    if conn.is_halted() {
                        return Ok(());
                    }
                )+
                Ok(())
            }

            #[allow(non_snake_case)]
            async fn after_response(&self, conn: &mut Conn) -> Result<()> {
                let ($(ref $name,)+) = *self;
                reverse_after_response!(conn, $($name)+);
                Ok(())
            }

            #[allow(non_snake_case)]
            fn name(&self) -> Cow<'static, str> {
                let ($(ref $name,)+) = *self;
                format!(concat!("(\n", $(
                        concat!("  {",stringify!($name) ,":},\n")
                ),*, ")"), $($name = ($name).name()),*).into()
            }
        }
    };
}

impl_client_handler_tuple! { A }
impl_client_handler_tuple! { A B }
impl_client_handler_tuple! { A B C }
impl_client_handler_tuple! { A B C D }
impl_client_handler_tuple! { A B C D E }
impl_client_handler_tuple! { A B C D E F }
impl_client_handler_tuple! { A B C D E F G }
impl_client_handler_tuple! { A B C D E F G H }
impl_client_handler_tuple! { A B C D E F G H I }
impl_client_handler_tuple! { A B C D E F G H I J }
impl_client_handler_tuple! { A B C D E F G H I J K }
impl_client_handler_tuple! { A B C D E F G H I J K L }
impl_client_handler_tuple! { A B C D E F G H I J K L M }
impl_client_handler_tuple! { A B C D E F G H I J K L M N }
impl_client_handler_tuple! { A B C D E F G H I J K L M N O }
