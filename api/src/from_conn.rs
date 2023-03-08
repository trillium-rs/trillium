use trillium::{async_trait, Conn};

///
#[async_trait]
pub trait FromConn: Send + Sync + Sized + 'static {
    ///
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
