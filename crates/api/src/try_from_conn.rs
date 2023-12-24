use crate::{ApiConnExt, FromConn};
use trillium::{async_trait, Conn, Handler};
/// Like FromConn, but with an Error.
///
/// If you want to use this directly, Error needs to be Handler.
///
/// If Error is not Handler, you can use `Result<T, E> as TryFromConn where T: TryFromConn<Error = E>`
///
/// If extraction is infallible, implement [`FromConn`].
#[async_trait]
pub trait TryFromConn: Send + Sync + Sized + 'static {
    /// The Error type. Tf this is a Handler, you can extract Self directly in a ApiHandler
    /// signature, and Error will be called on Conn if try_from_conn fails.
    type Error: Send + Sync + Sized + 'static;

    /// Attempt to extract Self from &mut Conn, returning Error in case of failure
    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Self::Error>;
}

#[async_trait]
impl TryFromConn for serde_json::Value {
    type Error = crate::Error;
    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Self::Error> {
        conn.deserialize().await
    }
}

#[async_trait]
impl<T: FromConn> TryFromConn for T {
    type Error = ();

    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Self::Error> {
        Self::from_conn(conn).await.ok_or(())
    }
}

macro_rules! impl_try_from_conn_tuple {
    ($($name:ident)+) => (
        #[async_trait]
        impl<$($name),*> TryFromConn for ($($name,)*) where $($name: TryFromConn, <$name as TryFromConn>::Error: Handler),* {
            type Error = Box<dyn Handler>;
            #[allow(non_snake_case)]
            async fn try_from_conn(conn: &mut Conn) -> Result<Self, Self::Error> {
                $(let $name = <$name as TryFromConn>::try_from_conn(conn)
                  .await
                  .map_err(|h| Box::new(h) as Box<dyn Handler>)?;)*
                Ok(($($name, )*))
            }
        }
    )
}

impl_try_from_conn_tuple! { A B }
impl_try_from_conn_tuple! { A B C }
impl_try_from_conn_tuple! { A B C D }
impl_try_from_conn_tuple! { A B C D E }
impl_try_from_conn_tuple! { A B C D E F }
impl_try_from_conn_tuple! { A B C D E F G }
impl_try_from_conn_tuple! { A B C D E F G H }
impl_try_from_conn_tuple! { A B C D E F G H I }
impl_try_from_conn_tuple! { A B C D E F G H I J }
impl_try_from_conn_tuple! { A B C D E F G H I J K }
impl_try_from_conn_tuple! { A B C D E F G H I J K L }
