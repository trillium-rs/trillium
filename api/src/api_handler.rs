use crate::FromConn;
use std::future::Future;
use std::marker::PhantomData;
use trillium::{async_trait, Conn, Handler};

// A trait for `async fn(conn: &mut Conn, additional: Additional) -> ReturnType`
pub trait MutBorrowConnWithBody<'conn, ReturnType, Additional>: Send + Sync + 'conn {
    /// the returned future
    type Fut: Future<Output = ReturnType> + Send + 'conn;
    /// executes this function
    fn call(&self, conn: &'conn mut Conn, additional: Additional) -> Self::Fut;
}

impl<'conn, Fun, Fut, ReturnType, Additional> MutBorrowConnWithBody<'conn, ReturnType, Additional>
    for Fun
where
    Fun: Fn(&'conn mut Conn, Additional) -> Fut + Send + Sync + 'conn,
    Fut: Future<Output = ReturnType> + Send + 'conn,
{
    type Fut = Fut;
    fn call(&self, conn: &'conn mut Conn, additional: Additional) -> Fut {
        self(conn, additional)
    }
}

/// A convenient way to define custom error handling behavior. This
/// handler will execute both on the `run` and `before_send` Handler
/// lifecycle hooks in order to ensure that it catches errors
/// regardless of where it is placed in the handler sequence.
#[derive(Debug)]
pub struct ApiHandler<F, OutputHandler, FromConn>(
    F,
    PhantomData<OutputHandler>,
    PhantomData<FromConn>,
);

impl<FromConnHandler, OutputHandler, Extracted>
    ApiHandler<FromConnHandler, OutputHandler, Extracted>
where
    FromConnHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, Extracted>,
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
    FromConnHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, Extracted>,
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
    FromConnHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, Extracted>,
    Extracted: FromConn,
    OutputHandler: Handler,
{
    ApiHandler::from(api_handler)
}

#[async_trait]
impl<FromConnHandler, OutputHandler, Extracted> Handler
    for ApiHandler<FromConnHandler, OutputHandler, Extracted>
where
    FromConnHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, Extracted>,
    Extracted: FromConn,
    OutputHandler: Handler,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        if let Some(extracted) = Extracted::from_conn(&mut conn).await {
            let output_handler = self.0.call(&mut conn, extracted).await;
            output_handler.run(conn).await
        } else {
            conn.halt()
        }
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        crate::default_error_handler::handle_error(conn)
    }
}
