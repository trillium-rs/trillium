use crate::ApiConnExt;
use trillium::{async_trait, Conn};

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
impl FromConn for serde_json::Value {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        let res = conn.deserialize_json::<serde_json::Value>().await;
        conn.store_error(res)
    }
}

#[cfg(feature = "querystrong")]
#[async_trait]
impl FromConn for querystrong::QueryStrong {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        Some(conn.querystring().parse().unwrap_or_default())
    }
}

macro_rules! impl_from_conn_tuple {
    ($($name:ident)+) => (
        #[async_trait]
        impl<$($name),*> FromConn for ($($name,)*) where $($name: FromConn),* {
            #[allow(non_snake_case)]
            async fn from_conn(conn: &mut Conn) -> Option<($($name,)*)> {
                $(let $name = <$name as FromConn>::from_conn(conn).await;)*
                Some(($($name?, )*))
            }
        }
    )
}

impl_from_conn_tuple! { A B }
impl_from_conn_tuple! { A B C }
impl_from_conn_tuple! { A B C D }
impl_from_conn_tuple! { A B C D E }
impl_from_conn_tuple! { A B C D E F }
impl_from_conn_tuple! { A B C D E F G }
impl_from_conn_tuple! { A B C D E F G H }
impl_from_conn_tuple! { A B C D E F G H I }
impl_from_conn_tuple! { A B C D E F G H I J }
impl_from_conn_tuple! { A B C D E F G H I J K }
impl_from_conn_tuple! { A B C D E F G H I J K L }
