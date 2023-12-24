use crate::FromConn;
use std::ops::{Deref, DerefMut};
use trillium::{async_trait, Conn, Handler};

/// State extractor
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct State<T>(pub T);

impl<T> State<T> {
    /// construct a new State
    pub fn new(t: T) -> Self {
        Self(t)
    }

    /// Unwrap this State
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<T> for State<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

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
impl<T> FromConn for State<T>
where
    T: Send + Sync + 'static,
{
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
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
