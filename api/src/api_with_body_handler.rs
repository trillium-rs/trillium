use crate::{default_error_handler::handle_error, ApiConnExt};
use serde::de::DeserializeOwned;
use std::{future::Future, marker::PhantomData};
use trillium::{async_trait, Conn, Handler};

/// A trillium handler that deserializes a body type from the request
/// and passes it into a function that mutably borrows Conn.
#[derive(Debug)]
pub struct ApiBodyHandler<HandlerFn, OutputHandler, RequestBody>(
    HandlerFn,
    PhantomData<OutputHandler>,
    PhantomData<RequestBody>,
);

/// A trait for `async fn(conn: &mut Conn, additional: Additional) -> ReturnType`
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

#[async_trait]
impl<ApiHandlerFn, OutputHandler, RequestBody> Handler
    for ApiBodyHandler<ApiHandlerFn, OutputHandler, RequestBody>
where
    ApiHandlerFn: for<'conn> MutBorrowConnWithBody<'conn, OutputHandler, RequestBody>,
    OutputHandler: Handler,
    RequestBody: DeserializeOwned + Send + Sync + 'static,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        match conn.deserialize::<RequestBody>().await {
            Ok(request_body) => {
                let output_handler = self.0.call(&mut conn, request_body).await;
                output_handler.run(conn).await
            }

            Err(error) => conn.with_state(error),
        }
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        handle_error(conn)
    }
}

impl<ApiHandlerFn, OutputHandler, RequestBody>
    ApiBodyHandler<ApiHandlerFn, OutputHandler, RequestBody>
where
    ApiHandlerFn: for<'conn> MutBorrowConnWithBody<'conn, OutputHandler, RequestBody>,
    OutputHandler: Handler,
    RequestBody: DeserializeOwned + Send + Sync + 'static,
{
    /// constructs a new `ApiBodyHandler` from the provided async
    /// function, which will have the form
    /// `async fn(&mut Conn, body: RequestBody) -> impl Handler`
    /// where `RequestBody` is any type that is [`DeserializeOwned`]
    pub fn new(api_handler_fn: ApiHandlerFn) -> Self {
        Self(api_handler_fn, PhantomData, PhantomData)
    }
}

/// constructs a new ApiBodyHandler from the provided async
/// function, which will have the form
/// `async fn(&mut Conn, body: RequestBody) -> impl Handler`
/// where `RequestBody` is any type that is [`DeserializeOwned`]
///
/// Shortcut for [`ApiBodyHandler::new`]

pub fn api_with_body<ApiHandlerFn, OutputHandler, RequestBody>(
    api_handler_fn: ApiHandlerFn,
) -> ApiBodyHandler<ApiHandlerFn, OutputHandler, RequestBody>
where
    ApiHandlerFn: for<'a> MutBorrowConnWithBody<'a, OutputHandler, RequestBody>,
    OutputHandler: Handler,
    RequestBody: DeserializeOwned + Send + Sync + 'static,
{
    ApiBodyHandler::new(api_handler_fn)
}
