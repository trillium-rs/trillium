use crate::FromConn;
use std::{future::Future, marker::PhantomData, sync::Arc};
use trillium::{async_trait, Conn, Handler, Info, Status, Upgrade};

// A trait for `async fn(conn: &mut Conn, additional: Additional) -> ReturnType`
pub trait MutBorrowConn<'conn, ReturnType, Additional>: Send + Sync + 'conn {
    /// the returned future
    type Fut: Future<Output = ReturnType> + Send + 'conn;
    /// executes this function
    fn call(&self, conn: &'conn mut Conn, additional: Additional) -> Self::Fut;
}

impl<'conn, Fun, Fut, ReturnType, Additional> MutBorrowConn<'conn, ReturnType, Additional> for Fun
where
    Fun: Fn(&'conn mut Conn, Additional) -> Fut + Send + Sync + 'conn,
    Fut: Future<Output = ReturnType> + Send + 'conn,
{
    type Fut = Fut;
    fn call(&self, conn: &'conn mut Conn, additional: Additional) -> Fut {
        self(conn, additional)
    }
}

/// An interface layer built on trillium
///
/// This handler provides the capacity to extract various components of a conn such as deserializing
/// a body, and supports returning handlers that will be called on the returned conn.
///
/// If [`ApiHandler`] encounters an error of any sort before the user-provided logic is executed, it
/// will put an [`Error`] into the conn's state. A default error handler is provided.
///
/// More documentation for this type is needed, hence the -rc semver on this crate
#[derive(Debug)]
pub struct ApiHandler<F, OutputHandler, FromConn>(
    F,
    PhantomData<OutputHandler>,
    PhantomData<FromConn>,
);

impl<FromConnHandler, OutputHandler, Extracted>
    ApiHandler<FromConnHandler, OutputHandler, Extracted>
where
    FromConnHandler: for<'a> MutBorrowConn<'a, OutputHandler, Extracted>,
    OutputHandler: Handler,
    Extracted: FromConn,
{
    /// constructs a new [`ApiFromConnHandler`] from the provided
    /// `async fn(&mut conn, FromConn) -> impl Handler`
    pub fn new(api_handler: FromConnHandler) -> Self {
        Self::from(api_handler)
    }
}

impl<FromConnHandler, OutputHandler, Extracted> From<FromConnHandler>
    for ApiHandler<FromConnHandler, OutputHandler, Extracted>
where
    FromConnHandler: for<'a> MutBorrowConn<'a, OutputHandler, Extracted>,
    OutputHandler: Handler,
    Extracted: FromConn,
{
    fn from(value: FromConnHandler) -> Self {
        Self(value, PhantomData, PhantomData)
    }
}

/// constructs a new [`ApiFromConnHandler`] from the provided
/// `async fn(&mut conn, FromConn) -> impl Handler`
///
/// convenience function for [`ApiFromConnHandler::new`]
pub fn api<FromConnHandler, OutputHandler, Extracted>(
    api_handler: FromConnHandler,
) -> ApiHandler<FromConnHandler, OutputHandler, Extracted>
where
    FromConnHandler: for<'a> MutBorrowConn<'a, OutputHandler, Extracted>,
    Extracted: FromConn,
    OutputHandler: Handler,
{
    ApiHandler::from(api_handler)
}

#[async_trait]
impl<FromConnHandler, OutputHandler, Extracted> Handler
    for ApiHandler<FromConnHandler, OutputHandler, Extracted>
where
    FromConnHandler: for<'a> MutBorrowConn<'a, OutputHandler, Extracted>,
    Extracted: FromConn,
    OutputHandler: Handler,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        if let Some(extracted) = Extracted::from_conn(&mut conn).await {
            let mut output_handler = self.0.call(&mut conn, extracted).await;
            if let Some(info) = conn.state_mut::<Info>() {
                output_handler.init(info).await;
            } else {
                output_handler.init(&mut Info::default()).await;
            }
            let mut conn = output_handler.run(conn).await;
            if conn.status().is_none() && conn.inner().response_body().is_some() {
                conn.set_status(Status::Ok);
            }
            conn.with_state(OutputHandlerWrapper(Arc::new(output_handler)))
        } else {
            conn.halt()
        }
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        if let Some(OutputHandlerWrapper(handler)) =
            conn.state::<OutputHandlerWrapper<OutputHandler>>().cloned()
        {
            handler.before_send(conn).await
        } else {
            crate::default_error_handler::handle_error(conn)
        }
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        upgrade
            .state()
            .get::<OutputHandlerWrapper<OutputHandler>>()
            .cloned()
            .map_or(false, |OutputHandlerWrapper(handler)| {
                handler.has_upgrade(upgrade)
            })
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        if let Some(OutputHandlerWrapper(handler)) = upgrade
            .state()
            .get::<OutputHandlerWrapper<OutputHandler>>()
            .cloned()
        {
            handler.upgrade(upgrade).await
        }
    }
}

struct OutputHandlerWrapper<OH>(Arc<OH>);
impl<OH> Clone for OutputHandlerWrapper<OH> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
