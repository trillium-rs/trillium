use trillium::{async_trait, Conn};

use crate::TryFromConn;

/// A trait to extract content from [`Conn`]s to be used as the second
/// argument to an api handler. Implement this for your types.
#[async_trait]
pub trait FromConn: Send + Sync + Sized + 'static {
    /// returning None from this will not call the api handler, but
    /// will halt the conn.
    async fn from_conn(conn: &mut Conn) -> Option<Self>;
}

#[async_trait]
impl FromConn for () {
    async fn from_conn(_: &mut Conn) -> Option<Self> {
        Some(())
    }
}

#[async_trait]
impl FromConn for String {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        conn.request_body_string().await.ok()
    }
}

#[async_trait]
impl FromConn for Vec<u8> {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        conn.request_body().await.read_bytes().await.ok()
    }
}

#[async_trait]
impl<E: FromConn> FromConn for Option<E> {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        Some(E::from_conn(conn).await)
    }
}

#[async_trait]
impl<T, E> FromConn for Result<T, E>
where
    T: TryFromConn<Error = E>,
    E: Send + Sync + 'static,
{
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        Some(T::try_from_conn(conn).await)
    }
}

#[async_trait]
impl FromConn for trillium::Headers {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        Some(conn.request_headers().clone())
    }
}

#[async_trait]
impl FromConn for trillium::Method {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        Some(conn.method())
    }
}
