use crate::TryFromConn;
use std::future::Future;
use trillium::Conn;

/// A trait to extract content from [`Conn`]s to be used as the second
/// argument to an api handler. Implement this for your types.
pub trait FromConn: Send + Sync + Sized + 'static {
    /// returning None from this will not call the api handler, but
    /// will halt the conn.
    fn from_conn(conn: &mut Conn) -> impl Future<Output = Option<Self>> + Send;
}

impl FromConn for () {
    async fn from_conn(_: &mut Conn) -> Option<Self> {
        Some(())
    }
}

impl<E: FromConn> FromConn for Option<E> {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        Some(E::from_conn(conn).await)
    }
}

impl<T, E> FromConn for Result<T, E>
where
    T: TryFromConn<Error = E>,
    E: Send + Sync + 'static,
{
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        Some(T::try_from_conn(conn).await)
    }
}

impl FromConn for trillium::Headers {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        Some(conn.request_headers().clone())
    }
}

impl FromConn for trillium::Method {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        Some(conn.method())
    }
}
