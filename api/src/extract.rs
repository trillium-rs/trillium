use trillium::{async_trait, Conn};

///
#[async_trait]
pub trait Extract: Send + Sync + Sized + 'static {
    ///
    async fn extract(conn: &mut Conn) -> Option<Self>;
}

#[async_trait]
impl Extract for () {
    async fn extract(_: &mut Conn) -> Option<Self> {
        Some(())
    }
}

#[async_trait]
impl Extract for String {
    async fn extract(conn: &mut Conn) -> Option<Self> {
        conn.request_body_string().await.ok()
    }
}

#[async_trait]
impl Extract for Vec<u8> {
    async fn extract(conn: &mut Conn) -> Option<Self> {
        conn.request_body().await.read_bytes().await.ok()
    }
}

#[async_trait]
impl<E: Extract> Extract for Option<E> {
    async fn extract(conn: &mut Conn) -> Option<Self> {
        Some(E::extract(conn).await)
    }
}

macro_rules! impl_extract_tuple {
    ($($name:ident)+) => (
        #[async_trait]
        impl<$($name),*> Extract for ($($name,)*) where $($name: Extract),* {
            #[allow(non_snake_case)]
            async fn extract(conn: &mut Conn) -> Option<($($name,)*)> {
                $(let $name = <$name as Extract>::extract(conn).await;)*
                Some(($($name?, )*))
            }
        }
    )
}

impl_extract_tuple! { A B }
impl_extract_tuple! { A B C }
impl_extract_tuple! { A B C D }
impl_extract_tuple! { A B C D E }
impl_extract_tuple! { A B C D E F }
impl_extract_tuple! { A B C D E F G }
impl_extract_tuple! { A B C D E F G H }
impl_extract_tuple! { A B C D E F G H I }
impl_extract_tuple! { A B C D E F G H I J }
impl_extract_tuple! { A B C D E F G H I J K }
impl_extract_tuple! { A B C D E F G H I J K L }
