use crate::{async_trait, Conn, Upgrade};
use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;

#[async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn run(&self, conn: Conn) -> Conn;

    async fn init(&mut self) {}

    async fn before_send(&self, conn: Conn) -> Conn {
        conn
    }

    fn has_upgrade(&self, _upgrade: &Upgrade) -> bool {
        false
    }

    async fn upgrade(&self, _upgrade: Upgrade) {
        unimplemented!("if has_upgrade returns true, you must also implement upgrade")
    }

    fn name(&self) -> Cow<'static, str> {
        std::any::type_name::<Self>().into()
    }
}

#[async_trait]
impl Handler for Box<dyn Handler> {
    async fn run(&self, conn: Conn) -> Conn {
        self.as_ref().run(conn).await
    }
    async fn init(&mut self) {
        self.as_mut().init().await
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
        self.as_ref().upgrade(upgrade).await
    }
}

impl std::fmt::Debug for Box<dyn Handler> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name().as_ref())
    }
}

#[async_trait]
impl<G: Handler> Handler for Arc<G> {
    async fn run(&self, conn: Conn) -> Conn {
        self.as_ref().run(conn).await
    }

    async fn init(&mut self) {
        Arc::<G>::get_mut(self)
            .expect("cannot call init when there are already clones of an Arc<Handler>")
            .init()
            .await
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
        self.as_ref().upgrade(upgrade).await
    }
}

#[async_trait]
impl<G: Handler> Handler for Vec<G> {
    async fn run(&self, mut conn: Conn) -> Conn {
        for handler in self {
            conn = handler.run(conn).await;
            if conn.is_halted() {
                break;
            }
        }
        conn
    }

    async fn init(&mut self) {
        for handler in self {
            handler.init().await;
        }
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        for handler in self.iter().rev() {
            conn = handler.before_send(conn).await
        }
        conn
    }

    fn name(&self) -> Cow<'static, str> {
        self.iter()
            .map(|v| v.name())
            .collect::<Vec<_>>()
            .join(",")
            .into()
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        self.iter().any(|g| g.has_upgrade(upgrade))
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        if let Some(handler) = self.iter().find(|g| g.has_upgrade(&upgrade)) {
            handler.upgrade(upgrade).await
        }
    }
}

#[derive(Default)]
pub struct Sequence(Vec<Box<dyn Handler>>);

impl Sequence {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn then(&mut self, handler: impl Handler) {
        self.0.push(Box::new(handler));
    }

    pub fn and(mut self, handler: impl Handler) -> Self {
        self.then(handler);
        self
    }
}

#[async_trait]
impl Handler for Sequence {
    async fn run(&self, conn: Conn) -> Conn {
        self.0.run(conn).await
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        self.0.before_send(conn).await
    }
    async fn init(&mut self) {
        self.0.init().await
    }

    fn name(&self) -> Cow<'static, str> {
        self.0.name()
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        self.0.has_upgrade(upgrade)
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        self.0.upgrade(upgrade).await
    }
}

#[async_trait]
impl<Fun, Fut> Handler for Fun
where
    Fun: Fn(Conn) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Conn> + Send + Sync + 'static,
{
    async fn run(&self, conn: Conn) -> Conn {
        (self)(conn).await
    }
}

#[async_trait]
impl Handler for String {
    async fn run(&self, conn: Conn) -> Conn {
        conn.body(&self[..])
    }
}

#[async_trait]
impl Handler for &'static str {
    async fn run(&self, conn: Conn) -> Conn {
        conn.body(*self)
    }
}
