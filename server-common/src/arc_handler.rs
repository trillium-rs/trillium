use std::{borrow::Cow, fmt::Debug, ops::Deref, sync::Arc};
use trillium::{Conn, Handler, Info, Upgrade};

pub(crate) struct ArcHandler<H>(Arc<H>);

impl<H> Clone for ArcHandler<H> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<H: Debug> Debug for ArcHandler<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&self.0, f)
    }
}

impl<H> Deref for ArcHandler<H> {
    type Target = H;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<H> AsRef<H> for ArcHandler<H> {
    fn as_ref(&self) -> &H {
        &self.0
    }
}

impl<H: Handler> ArcHandler<H> {
    pub fn new(handler: H) -> Self {
        Self(Arc::new(handler))
    }
}

impl<H: Handler> Handler for ArcHandler<H> {
    async fn run(&self, conn: Conn) -> Conn {
        self.as_ref().run(conn).await
    }

    async fn init(&mut self, _info: &mut Info) {
        panic!("this should never be executed")
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        self.as_ref().before_send(conn).await
    }

    fn name(&self) -> Cow<'static, str> {
        self.as_ref().name()
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        self.as_ref().has_upgrade(upgrade)
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        self.as_ref().upgrade(upgrade).await;
    }
}
