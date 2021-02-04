use crate::{async_trait, Conn, Upgrade};
use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;

#[async_trait]
pub trait Grain: Send + Sync + 'static {
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
impl Grain for Box<dyn Grain> {
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

#[async_trait]
impl<G: Grain> Grain for Arc<G> {
    async fn run(&self, conn: Conn) -> Conn {
        self.as_ref().run(conn).await
    }

    async fn init(&mut self) {
        Arc::<G>::get_mut(self)
            .expect("cannot call init when there are already clones of an Arc<Grain>")
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
impl<G: Grain> Grain for Vec<G> {
    async fn run(&self, mut conn: Conn) -> Conn {
        for grain in self {
            conn = grain.run(conn).await;
            if conn.is_halted() {
                break;
            }
        }
        conn
    }

    async fn init(&mut self) {
        for grain in self {
            grain.init().await;
        }
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        for grain in self.iter().rev() {
            conn = grain.before_send(conn).await
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
        if let Some(grain) = self.iter().find(|g| g.has_upgrade(&upgrade)) {
            grain.upgrade(upgrade).await
        }
    }
}

#[derive(Default)]
pub struct Sequence(Vec<Box<dyn Grain>>);

impl Sequence {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn then(&mut self, grain: impl Grain) {
        self.0.push(Box::new(grain));
    }

    pub fn and(mut self, grain: impl Grain) -> Self {
        self.then(grain);
        self
    }
}

#[async_trait]
impl Grain for Sequence {
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
impl<Fun, Fut> Grain for Fun
where
    Fun: Fn(Conn) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Conn> + Send + Sync + 'static,
{
    async fn run(&self, conn: Conn) -> Conn {
        (self)(conn).await
    }
}

#[async_trait]
impl Grain for String {
    async fn run(&self, conn: Conn) -> Conn {
        conn.body(&self[..])
    }
}

#[async_trait]
impl Grain for &'static str {
    async fn run(&self, conn: Conn) -> Conn {
        conn.body(*self)
    }
}
