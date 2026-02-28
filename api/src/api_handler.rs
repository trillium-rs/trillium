use crate::TryFromConn;
use std::{future::Future, marker::PhantomData, sync::Arc};
use trillium::{Conn, Handler, Info, Status, Upgrade};

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
pub struct ApiHandler<F, OutputHandler, TryFromConn>(
    F,
    PhantomData<OutputHandler>,
    PhantomData<TryFromConn>,
);

impl<TryFromConnHandler, OutputHandler, Extracted>
    ApiHandler<TryFromConnHandler, OutputHandler, Extracted>
where
    TryFromConnHandler: for<'a> MutBorrowConn<'a, OutputHandler, Extracted>,
    OutputHandler: Handler,
    Extracted: TryFromConn,
{
    /// constructs a new [`ApiTryFromConnHandler`] from the provided
    /// `async fn(&mut conn, TryFromConn) -> impl Handler`
    pub fn new(api_handler: TryFromConnHandler) -> Self {
        Self::from(api_handler)
    }
}

impl<TryFromConnHandler, OutputHandler, Extracted> From<TryFromConnHandler>
    for ApiHandler<TryFromConnHandler, OutputHandler, Extracted>
where
    TryFromConnHandler: for<'a> MutBorrowConn<'a, OutputHandler, Extracted>,
    OutputHandler: Handler,
    Extracted: TryFromConn,
{
    fn from(value: TryFromConnHandler) -> Self {
        Self(value, PhantomData, PhantomData)
    }
}

/// constructs a new [`ApiTryFromConnHandler`] from the provided
/// `async fn(&mut conn, TryFromConn) -> impl Handler`
///
/// convenience function for [`ApiTryFromConnHandler::new`]
pub fn api<TryFromConnHandler, OutputHandler, Extracted>(
    api_handler: TryFromConnHandler,
) -> ApiHandler<TryFromConnHandler, OutputHandler, Extracted>
where
    TryFromConnHandler: for<'a> MutBorrowConn<'a, OutputHandler, Extracted>,
    Extracted: TryFromConn,
    OutputHandler: Handler,
{
    ApiHandler::from(api_handler)
}

impl<TryFromConnHandler, OutputHandler, Extracted> Handler
    for ApiHandler<TryFromConnHandler, OutputHandler, Extracted>
where
    TryFromConnHandler: for<'a> MutBorrowConn<'a, OutputHandler, Extracted>,
    Extracted: TryFromConn,
    Extracted::Error: Handler,
    OutputHandler: Handler,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        let mut output_handler: Result<OutputHandler, <Extracted as TryFromConn>::Error> =
            match Extracted::try_from_conn(&mut conn).await {
                Ok(extracted) => Ok(self.0.call(&mut conn, extracted).await),
                Err(error_handler) => Err(error_handler),
            };

        if let Some(info) = conn.state_mut::<Info>() {
            output_handler.init(info).await;
        } else {
            output_handler.init(&mut Info::default()).await;
        }
        let mut conn = output_handler.run(conn).await;
        if conn.status().is_none() && conn.inner().response_body().is_some() {
            conn.set_status(Status::Ok);
        }
        conn.with_state(OutputHandlerWrapper(
            Arc::new(output_handler),
            PhantomData::<Self>,
        ))
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        match conn
            .state::<OutputHandlerWrapper<Self, OutputHandler, <Extracted as TryFromConn>::Error>>()
            .cloned()
        {
            Some(OutputHandlerWrapper(handler, _)) => handler.before_send(conn).await,
            _ => conn,
        }
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        upgrade
            .state()
            .get::<OutputHandlerWrapper<Self, OutputHandler, <Extracted as TryFromConn>::Error>>()
            .cloned()
            .map_or(false, |OutputHandlerWrapper(handler, _)| {
                handler.has_upgrade(upgrade)
            })
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        if let Some(OutputHandlerWrapper(handler, _)) = upgrade
            .state()
            .get::<OutputHandlerWrapper<Self, OutputHandler, <Extracted as TryFromConn>::Error>>()
            .cloned()
        {
            handler.upgrade(upgrade).await
        }
    }
}

struct OutputHandlerWrapper<TFC, OH, EH>(Arc<Result<OH, EH>>, PhantomData<TFC>);

impl<TFC, OH, EH> Clone for OutputHandlerWrapper<TFC, OH, EH> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0), self.1)
    }
}
