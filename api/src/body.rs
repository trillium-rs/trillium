use crate::{ApiConnExt, TryFromConn};
use serde::{de::DeserializeOwned, Serialize};
use std::ops::{Deref, DerefMut};
use trillium::{async_trait, Conn, Handler};

/// Body extractor
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct Body<T>(pub T);

impl<T> Body<T> {
    /// construct a new Body
    pub fn new(t: T) -> Self {
        Self(t)
    }

    /// Unwrap this Body
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<T> for Body<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> Deref for Body<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Body<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[async_trait]
impl<T> TryFromConn for Body<T>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    type Error = crate::Error;
    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Self::Error> {
        conn.deserialize().await.map(Self)
    }
}

#[async_trait]
impl<T> Handler for Body<T>
where
    T: Serialize + Send + Sync + 'static,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        let result = conn.serialize(&self.0).await;
        conn.store_error(result);
        conn
    }
}
