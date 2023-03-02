use crate::api_with_body_handler::MutBorrowConnWithBody;
use std::marker::PhantomData;
use trillium::{async_trait, Conn, Handler};

/// A convenient way to define custom error handling behavior. This
/// handler will execute both on the `run` and `before_send` Handler
/// lifecycle hooks in order to ensure that it catches errors
/// regardless of where it is placed in the handler sequence.
#[derive(Debug)]
pub struct ApiStateHandler<F, OutputHandler, State>(
    F,
    PhantomData<OutputHandler>,
    PhantomData<State>,
);

impl<StateHandler, OutputHandler, State> ApiStateHandler<StateHandler, OutputHandler, State>
where
    StateHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, State>,
    OutputHandler: Handler,
    State: Send + Sync + 'static,
{
    /// constructs a new [`ApiStateHandler`] from the provided
    /// `async fn(&mut conn, State) -> impl Handler`
    pub fn new(error_handler: StateHandler) -> Self {
        ApiStateHandler(error_handler, PhantomData, PhantomData)
    }
}

/// constructs a new [`ApiStateHandler`] from the provided
/// `async fn(&mut conn, State) -> impl Handler`
///
/// convenience function for [`ApiStateHandler::new`]
pub fn state_handler<StateHandler, OutputHandler, State>(
    error_handler: StateHandler,
) -> ApiStateHandler<StateHandler, OutputHandler, State>
where
    StateHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, State>,
    State: Send + Sync + 'static,
    OutputHandler: Handler,
{
    ApiStateHandler::new(error_handler)
}

#[async_trait]
impl<StateHandler, OutputHandler, State> Handler
    for ApiStateHandler<StateHandler, OutputHandler, State>
where
    StateHandler: for<'a> MutBorrowConnWithBody<'a, OutputHandler, State>,
    State: Send + Sync + 'static,
    OutputHandler: Handler,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        let Some(error) = conn.take_state::<State>() else { return conn };
        let handler = self.0.call(&mut conn, error).await;
        handler.run(conn).await
    }
}
