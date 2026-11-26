use std::{
    any::Any,
    borrow::Cow,
    fmt::{self, Debug, Formatter},
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
};
use trillium::{Conn, Handler, Info, Upgrade};

trait ObjectSafeHandler: Any + Send + Sync + 'static {
    #[must_use]
    fn run<'handler, 'fut>(
        &'handler self,
        conn: Conn,
    ) -> Pin<Box<dyn Future<Output = Conn> + Send + 'fut>>
    where
        'handler: 'fut,
        Self: 'fut;
    #[must_use]
    fn before_send<'handler, 'fut>(
        &'handler self,
        conn: Conn,
    ) -> Pin<Box<dyn Future<Output = Conn> + Send + 'fut>>
    where
        'handler: 'fut,
        Self: 'fut;
    fn has_upgrade(&self, upgrade: &Upgrade) -> bool;
    #[must_use]
    fn upgrade<'handler, 'fut>(
        &'handler self,
        upgrade: Upgrade,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'fut>>
    where
        'handler: 'fut,
        Self: 'fut;
    fn name(&self) -> Cow<'static, str>;
}
impl<H: Handler> ObjectSafeHandler for H {
    fn run<'handler, 'fut>(
        &'handler self,
        conn: Conn,
    ) -> Pin<Box<dyn Future<Output = Conn> + Send + 'fut>>
    where
        'handler: 'fut,
        Self: 'fut,
    {
        Box::pin(async move { Handler::run(self, conn).await })
    }

    fn before_send<'handler, 'fut>(
        &'handler self,
        conn: Conn,
    ) -> Pin<Box<dyn Future<Output = Conn> + Send + 'fut>>
    where
        'handler: 'fut,
        Self: 'fut,
    {
        Box::pin(async move { Handler::before_send(self, conn).await })
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        Handler::has_upgrade(self, upgrade)
    }

    fn upgrade<'handler, 'fut>(
        &'handler self,
        upgrade: Upgrade,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'fut>>
    where
        'handler: 'fut,
        Self: 'fut,
    {
        Box::pin(async move {
            Handler::upgrade(self, upgrade).await;
        })
    }

    fn name(&self) -> Cow<'static, str> {
        Handler::name(self)
    }
}

pub(crate) static DEFAULT_MISSING_HANDLER: OnceLock<MissingHandler> = OnceLock::new();

/// A type-erased handler that gets called whenever a FromConn is None.
#[derive(Clone)]
pub struct MissingHandler(Arc<dyn ObjectSafeHandler>);
impl Debug for MissingHandler {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("BoxedHandler").field(&self.0.name()).finish()
    }
}

/// Set the missing handler for this application
pub fn missing_handler(handler: impl Handler) -> impl Handler {
    struct MissingHandlerSetter<H>(Option<H>);
    impl<H: Handler> Handler for MissingHandlerSetter<H> {
        async fn run(&self, conn: Conn) -> Conn {
            conn
        }

        async fn init(&mut self, info: &mut Info) {
            if let Some(mut handler) = self.0.take() {
                handler.init(info).await;
                info.insert_shared_state(MissingHandler(Arc::new(handler)));
            }
        }
    }

    MissingHandlerSetter(Some(handler))
}

impl MissingHandler {
    pub(crate) fn new(handler: impl Handler) -> Self {
        Self(Arc::new(handler))
    }
}

impl Handler for MissingHandler {
    async fn run(&self, conn: Conn) -> Conn {
        self.0.run(conn).await
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        self.0.before_send(conn).await
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        self.0.has_upgrade(upgrade)
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        self.0.upgrade(upgrade).await;
    }

    fn name(&self) -> Cow<'static, str> {
        self.0.name()
    }
}
