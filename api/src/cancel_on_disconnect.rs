use crate::TryFromConn;
use std::{future::Future, marker::PhantomData, sync::Arc};
use trillium::{async_trait, Conn, Handler, Info, Status, Upgrade};

/// A struct that cancels a handler if the client disconnects.
///
/// Note that the conn is not available to this handler, and any properties of the request needed
/// for execution must be extracted through [`FromConn`] or [`TryFromConn`] arguments
#[derive(Debug)]
pub struct CancelOnDisconnect<F, OutputHandler, TryFromConn>(
    F,
    PhantomData<OutputHandler>,
    PhantomData<TryFromConn>,
);

impl<F, OH, TFC, Fut> CancelOnDisconnect<F, OH, TFC>
where
    F: Fn(TFC) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = OH> + Send + 'static,
    OH: Handler,
    TFC: TryFromConn,
    TFC::Error: Handler,
{
    /// Construct a new CancelOnDisconnect handler
    pub fn new(handler: F) -> Self {
        CancelOnDisconnect(handler, PhantomData, PhantomData)
    }
}

/// Construct a new [`CancelOnDisconnect`] handler.
///
/// Alias for [`CancelOnDisconnect::new`]
pub fn cancel_on_disconnect<F, OH, TFC, Fut>(handler: F) -> CancelOnDisconnect<F, OH, TFC>
where
    F: Fn(TFC) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = OH> + Send + 'static,
    OH: Handler,
    TFC: TryFromConn,
    TFC::Error: Handler,
{
    CancelOnDisconnect(handler, PhantomData, PhantomData)
}

#[async_trait]
impl<F, OutputHandler, TFC, Fut> Handler for CancelOnDisconnect<F, OutputHandler, TFC>
where
    F: Fn(TFC) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = OutputHandler> + Send + 'static,
    OutputHandler: Handler,
    TFC: TryFromConn,
    TFC::Error: Handler,
{
    async fn run(&self, mut conn: Conn) -> Conn {
        let mut output_handler: Result<OutputHandler, <TFC as TryFromConn>::Error> =
            match TFC::try_from_conn(&mut conn).await {
                Ok(extracted) => {
                    let Some(ret) = conn.cancel_on_disconnect(self.0(extracted)).await else {
                        log::info!("client disconnected");
                        return conn;
                    };
                    Ok(ret)
                }
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
        if let Some(OutputHandlerWrapper(handler, _)) = conn
            .state::<OutputHandlerWrapper<Self, OutputHandler, <TFC as TryFromConn>::Error>>()
            .cloned()
        {
            handler.before_send(conn).await
        } else {
            conn
        }
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        upgrade
            .state()
            .get::<OutputHandlerWrapper<Self, OutputHandler, <TFC as TryFromConn>::Error>>()
            .cloned()
            .map_or(false, |OutputHandlerWrapper(handler, _)| {
                handler.has_upgrade(upgrade)
            })
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        if let Some(OutputHandlerWrapper(handler, _)) = upgrade
            .state()
            .get::<OutputHandlerWrapper<Self, OutputHandler, <TFC as TryFromConn>::Error>>()
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
