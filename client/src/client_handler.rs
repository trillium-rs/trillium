use std::borrow::Cow;

use crate::Conn;
use trillium_server_common::async_trait;

#[async_trait]
pub trait ClientHandler: std::fmt::Debug + Send + Sync + 'static {
    async fn before(&self, conn: &mut Conn) -> crate::Result<()> {
        let _ = conn;
        Ok(())
    }

    async fn after(&self, conn: &mut Conn) -> crate::Result<()> {
        let _ = conn;
        Ok(())
    }

    fn name(&self) -> Cow<'static, str> {
        std::any::type_name::<Self>().into()
    }
}

impl ClientHandler for () {}

#[async_trait]
impl<H: ClientHandler> ClientHandler for Option<H> {
    async fn before(&self, conn: &mut Conn) -> crate::Result<()> {
        match self {
            Some(h) => h.before(conn).await,
            None => Ok(()),
        }
    }

    async fn after(&self, conn: &mut Conn) -> crate::Result<()> {
        match self {
            Some(h) => h.after(conn).await,
            None => Ok(()),
        }
    }

    fn name(&self) -> Cow<'static, str> {
        match self {
            Some(h) => h.name(),
            None => "None".into(),
        }
    }
}

macro_rules! impl_handler_tuple {
    ($($name:ident)+) => (
        #[async_trait]
        impl<$($name),*> ClientHandler for ($($name,)*) where $($name: ClientHandler),* {
            #[allow(non_snake_case)]
            async fn before(&self, conn: &mut Conn) -> crate::Result<()> {
                let ($(ref $name,)*) = *self;
                $(
                    log::trace!("running {}", ($name).name());
                    ($name).before(conn).await?;
                )*
                Ok(())
            }
            #[allow(non_snake_case)]
            async fn after(&self, conn: &mut Conn) -> crate::Result<()> {
                let ($(ref $name,)*) = *self;
                $(
                    log::trace!("running {}", ($name).name());
                    ($name).after(conn).await?;
                )*
                Ok(())
            }

            #[allow(non_snake_case)]
            fn name(&self) -> Cow<'static, str> {
                let ($(ref $name,)*) = *self;
                format!(concat!("(\n", $(
                    concat!("  {",stringify!($name) ,":},\n")
                ),*, ")"), $($name = ($name).name()),*).into()
            }
        }
    );
}
impl_handler_tuple! { A }
impl_handler_tuple! { A B }
impl_handler_tuple! { A B C }
impl_handler_tuple! { A B C D }
impl_handler_tuple! { A B C D E }
impl_handler_tuple! { A B C D E F }
impl_handler_tuple! { A B C D E F G }
impl_handler_tuple! { A B C D E F G H }
impl_handler_tuple! { A B C D E F G H I }
impl_handler_tuple! { A B C D E F G H I J }
impl_handler_tuple! { A B C D E F G H I J K }
impl_handler_tuple! { A B C D E F G H I J K L }
