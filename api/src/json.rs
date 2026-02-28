use crate::{ApiConnExt, TryFromConn};
use serde::{Serialize, de::DeserializeOwned};
use std::ops::{Deref, DerefMut};
use trillium::{Conn, Handler};

/// A newtype wrapper struct for any [`serde::Serialize`] type. Note
/// that this currently must own the serializable type.
/// Body extractor
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Ord, PartialOrd, Hash)]
pub struct Json<T>(pub T);

impl<T> Json<T> {
    /// construct a new Json
    pub fn new(t: T) -> Self {
        Self(t)
    }

    /// Unwrap this Json
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<T> for Json<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> Deref for Json<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for Json<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl<Serializable> Handler for Json<Serializable>
where
    Serializable: Serialize + Send + Sync + 'static,
{
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_json(&self.0)
    }
}

impl<T> TryFromConn for Json<T>
where
    T: DeserializeOwned + Send + Sync + 'static,
{
    type Error = crate::Error;

    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Self::Error> {
        conn.deserialize_json().await.map(Self)
    }
}
