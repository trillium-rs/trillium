use crate::{Conn, Handler, Info, Upgrade};
use std::{
    any::Any,
    borrow::Cow,
    fmt::{self, Debug, Formatter},
    future::Future,
    pin::Pin,
};

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
    fn init<'handler, 'info, 'fut>(
        &'handler mut self,
        info: &'info mut Info,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'fut>>
    where
        'handler: 'fut,
        'info: 'fut,
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
    fn as_box_any(self: Box<Self>) -> Box<dyn Any>;
    fn as_any(&self) -> &dyn Any;
    fn as_mut_any(&mut self) -> &mut dyn Any;
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

    fn init<'handler, 'info, 'fut>(
        &'handler mut self,
        info: &'info mut Info,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'fut>>
    where
        'handler: 'fut,
        'info: 'fut,
        Self: 'fut,
    {
        Box::pin(async move {
            Handler::init(self, info).await;
        })
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

    fn as_box_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_mut_any(&mut self) -> &mut dyn Any {
        self
    }
}

/// A type-erased handler
pub struct BoxedHandler(Box<dyn ObjectSafeHandler>);
impl Debug for BoxedHandler {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("BoxedHandler").field(&self.0.name()).finish()
    }
}

impl BoxedHandler {
    /// Constructs a new `BoxedHandler`
    #[must_use]
    pub fn new(handler: impl Handler) -> Self {
        Self(Box::new(handler))
    }

    /// Determine if this `BoxedHandler` is the specified type
    pub fn is<T: Any + 'static>(&self) -> bool {
        self.as_any().is::<T>()
    }

    /// Attempt to transform this `BoxedHandler` into the specified type
    ///
    /// # Errors
    ///
    /// Downcast returns the `BoxedHandler` as an error if it does not contain the provided type
    #[must_use = "downcast takes the handler, so you must use it"]
    #[allow(clippy::missing_panics_doc)]
    pub fn downcast<T: Any + 'static>(self) -> Result<T, Self> {
        if self.0.as_any().is::<T>() {
            Ok(*self.0.as_box_any().downcast().unwrap())
        } else {
            Err(self)
        }
    }

    /// Attempt to borrow this `BoxedHandler` as the provided type, returning None if it does not
    /// contain the type
    pub fn downcast_ref<T: Any + 'static>(&self) -> Option<&T> {
        self.0.as_any().downcast_ref()
    }

    /// Attempt to mutably borrow this `BoxedHandler` as the provided type, returning None if it
    /// does not contain the type
    pub fn downcast_mut<T: Any + 'static>(&mut self) -> Option<&mut T> {
        self.0.as_mut_any().downcast_mut()
    }
}

impl Handler for BoxedHandler {
    async fn run(&self, conn: Conn) -> Conn {
        self.0.run(conn).await
    }

    async fn init(&mut self, info: &mut Info) {
        self.0.init(info).await;
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
