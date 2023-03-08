use crate::api_with_body_handler::MutBorrowConnWithBody;
use crate::extract::Extract;
use std::marker::PhantomData;
use trillium::{async_trait, Conn, Handler};

/// A convenient way to define custom error handling behavior. This
/// handler will execute both on the `run` and `before_send` Handler
/// lifecycle hooks in order to ensure that it catches errors
/// regardless of where it is placed in the handler sequence.
#[derive(Debug)]
pub struct ApiExtractHandler<F, OutputHandler, Extract>(
    F,
    PhantomData<OutputHandler>,
    PhantomData<Extract>,
);

impl<ExtractHandler, OutputHandler, Extractor>
    ApiExtractHandler<ExtractHandler, OutputHandler, Extractor>
where
    ExtractHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, Extractor>,
    OutputHandler: Handler,
    Extractor: Extract,
{
    /// constructs a new [`ApiExtractHandler`] from the provided
    /// `async fn(&mut conn, Extract) -> impl Handler`
    pub fn new(error_handler: ExtractHandler) -> Self {
        ApiExtractHandler(error_handler, PhantomData, PhantomData)
    }
}

/// constructs a new [`ApiExtractHandler`] from the provided
/// `async fn(&mut conn, Extract) -> impl Handler`
///
/// convenience function for [`ApiExtractHandler::new`]
pub fn extract_handler<ExtractHandler, OutputHandler, Extractor>(
    error_handler: ExtractHandler,
) -> ApiExtractHandler<ExtractHandler, OutputHandler, Extractor>
where
    ExtractHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, Extractor>,
    Extractor: Extract,
    OutputHandler: Handler,
{
    ApiExtractHandler::new(error_handler)
}

#[async_trait]
impl<ExtractHandler, OutputHandler, Extractor> Handler
    for ApiExtractHandler<ExtractHandler, OutputHandler, Extractor>
where
    ExtractHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, Extractor>,
    Extractor: Extract,
    OutputHandler: Handler,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        if let Some(extracted) = Extractor::extract(&mut conn).await {
            let handler = self.0.call(&mut conn, extracted).await;
            handler.run(conn).await
        } else {
            conn
        }
    }
}
