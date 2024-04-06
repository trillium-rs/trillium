use std::marker::PhantomData;

use crate::{api_with_body_handler::MutBorrowConnWithBody, Error};
use trillium::{Conn, Handler};

/// A convenient way to define custom error handling behavior. This
/// handler will execute both on the `run` and `before_send` Handler
/// lifecycle hooks in order to ensure that it catches errors
/// regardless of where it is placed in the handler sequence.
#[derive(Debug)]
pub struct ApiErrorHandler<F, OutputHandler>(F, PhantomData<OutputHandler>);

impl<ErrorHandler, OutputHandler> ApiErrorHandler<ErrorHandler, OutputHandler>
where
    ErrorHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, Error>,
    OutputHandler: Handler,
{
    /// constructs a new [`ApiErrorHandler`] from the provided
    /// `async fn(&mut conn, Error) -> impl Handler`
    pub fn new(error_handler: ErrorHandler) -> Self {
        ApiErrorHandler(error_handler, PhantomData)
    }
}

/// constructs a new [`ApiErrorHandler`] from the provided
/// `async fn(&mut conn, Error) -> impl Handler`
///
/// convenience function for [`ApiErrorHandler::new`]
pub fn api_error<ErrorHandler, OutputHandler>(
    error_handler: ErrorHandler,
) -> ApiErrorHandler<ErrorHandler, OutputHandler>
where
    ErrorHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, Error>,
    OutputHandler: Handler,
{
    ApiErrorHandler::new(error_handler)
}

impl<ErrorHandler, OutputHandler> Handler for ApiErrorHandler<ErrorHandler, OutputHandler>
where
    ErrorHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, Error>,
    OutputHandler: Handler,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        let Some(error) = conn.take_state::<Error>() else {
            return conn;
        };
        let handler = self.0.call(&mut conn, error).await;
        handler.run(conn).await
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        let Some(error) = conn.take_state::<Error>() else {
            return conn;
        };
        let handler = self.0.call(&mut conn, error).await;
        handler.run(conn).await
    }
}
