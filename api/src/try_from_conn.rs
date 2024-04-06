use crate::{ApiConnExt, FromConn};
use std::future::Future;
use trillium::{BoxedHandler, Conn, Handler};

/// Like FromConn, but with an Error.
///
/// If you want to use this directly, Error needs to be Handler.
///
/// If Error is not Handler, you can use `Result<T, E> as TryFromConn where T: TryFromConn<Error = E>`
///
/// If extraction is infallible, implement [`FromConn`].
pub trait TryFromConn: Send + Sync + Sized + 'static {
    /// The Error type. Tf this is a Handler, you can extract Self directly in a ApiHandler
    /// signature, and Error will be called on Conn if try_from_conn fails.
    type Error: Send + Sync + Sized + 'static;

    /// Attempt to extract Self from &mut Conn, returning Error in case of failure
    fn try_from_conn(conn: &mut Conn) -> impl Future<Output = Result<Self, Self::Error>> + Send;
}

impl TryFromConn for serde_json::Value {
    type Error = crate::Error;
    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Self::Error> {
        conn.deserialize().await
    }
}

impl<T: FromConn> TryFromConn for T {
    type Error = ();

    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Self::Error> {
        Self::from_conn(conn).await.ok_or(())
    }
}

impl TryFromConn for Vec<u8> {
    type Error = crate::Error;
    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Self::Error> {
        conn.request_body()
            .await
            .read_bytes()
            .await
            .map_err(Into::into)
    }
}

impl TryFromConn for String {
    type Error = crate::Error;
    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Self::Error> {
        conn.request_body_string().await.map_err(Into::into)
    }
}

#[cfg(feature = "url")]
impl TryFromConn for url::Url {
    type Error = trillium::Status;
    async fn try_from_conn(conn: &mut Conn) -> Result<Self, Self::Error> {
        let path = conn.path();
        let host = conn
            .request_headers()
            .get_str(trillium::KnownHeaderName::Host)
            .ok_or(trillium::Status::BadRequest)?;
        let proto = if conn.is_secure() { "https" } else { "http" };
        url::Url::parse(&format!("{proto}://{host}{path}"))
            .map_err(|_| trillium::Status::BadRequest)
    }
}

macro_rules! impl_try_from_conn_tuple {
    ($($name:ident)+) => (
        impl<$($name),*> TryFromConn for ($($name,)*) where $($name: TryFromConn, <$name as TryFromConn>::Error: Handler),* {
            type Error = BoxedHandler;
            #[allow(non_snake_case)]
            async fn try_from_conn(conn: &mut Conn) -> Result<Self, Self::Error> {
                $(let $name = <$name as TryFromConn>::try_from_conn(conn)
                  .await
                  .map_err(|h| BoxedHandler::new(h))?;)*
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
