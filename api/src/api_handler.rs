use std::{future::Future, marker::PhantomData};

use crate::default_error_handler::handle_error;
use trillium::{async_trait, Conn, Handler};

/// A trillium handler that mutably borrows `Conn` and returns another
/// handler that will be executed on `Conn`.
#[derive(Debug)]
pub struct ApiHandler<ApiHandlerFn, OutputHandler>(ApiHandlerFn, PhantomData<OutputHandler>);

/// A function that mutably borrows Conn and returns a Future
pub trait MutBorrowConn<'conn, OutputHandler>: Send + Sync + 'conn {
    /// The future that is returned
    type Fut: Future<Output = OutputHandler> + Send + 'conn;
    /// Execute this function
    fn call(&self, conn: &'conn mut Conn) -> Self::Fut;
}

impl<'conn, Fun, Fut, Output> MutBorrowConn<'conn, Output> for Fun
where
    Fun: Fn(&'conn mut Conn) -> Fut + Send + Sync + 'conn,
    Fut: Future<Output = Output> + Send + 'conn,
    Output: Handler,
{
    type Fut = Fut;
    fn call(&self, conn: &'conn mut Conn) -> Fut {
        self(conn)
    }
}

#[async_trait]
impl<MutBorrowHandler, OutputHandler> Handler for ApiHandler<MutBorrowHandler, OutputHandler>
where
    MutBorrowHandler: for<'conn> MutBorrowConn<'conn, OutputHandler>,
    OutputHandler: Handler,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        let output_handler = self.0.call(&mut conn).await;
        output_handler.run(conn).await
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        handle_error(conn)
    }
}

impl<MutBorrowHandler, OutputHandler> ApiHandler<MutBorrowHandler, OutputHandler>
where
    MutBorrowHandler: for<'a> MutBorrowConn<'a, OutputHandler>,
    OutputHandler: Handler,
{
    /// constructs a new ApiHandler from the provided
    /// handler_function, which will have the general shape
    /// `async fn(conn: &mut Conn) -> impl Handler`
    pub fn new(handler_function: MutBorrowHandler) -> Self {
        Self(handler_function, PhantomData)
    }
}

/// constructs a new ApiHandler from the provided
/// handler_function, which will have the general shape
/// `async fn(conn: &mut Conn) -> impl Handler`
///
/// Shortcut for [`ApiHandler::new`]
pub fn api<F, O>(f: F) -> ApiHandler<F, O>
where
    F: for<'a> MutBorrowConn<'a, O>,
    O: Handler,
{
    ApiHandler::new(f)
}
