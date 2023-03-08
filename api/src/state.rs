use crate::Extract;
use std::ops::{Deref, DerefMut};
use trillium::{async_trait, Conn, Handler};

/// State extractor
#[derive(Debug)]
pub struct State<T>(pub T);

impl<T> Deref for State<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for State<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[async_trait]
impl<T> Extract for State<T>
where
    T: std::fmt::Debug + Send + Sync + 'static,
{
    async fn extract(conn: &mut Conn) -> Option<Self> {
        conn.take_state::<T>().map(Self)
    }
}

#[async_trait]
impl<T> Handler for State<T>
where
    T: Clone + Send + Sync + 'static,
{
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(self.0.clone())
    }
}
