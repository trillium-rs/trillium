use lamedh_runtime::{Context, Handler as AwsHandler};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use trillium::{Conn, Handler};
use trillium_http::{Conn as HttpConn, Synthetic};

mod context;
mod request;
mod response;

use context::LambdaContext;
use request::LambdaRequest;
use response::{AlbMultiHeadersResponse, AlbResponse, LambdaResponse};

pub use context::LambdaConnExt;

struct HandlerWrapper<G>(Arc<G>);

impl<G: Handler> AwsHandler<LambdaRequest, LambdaResponse> for HandlerWrapper<G> {
    type Error = std::io::Error;
    type Fut = Pin<Box<dyn Future<Output = Result<LambdaResponse, Self::Error>> + Send + 'static>>;

    fn call(&mut self, request: LambdaRequest, context: Context) -> Self::Fut {
        Box::pin(handler_fn(request, context, Arc::clone(&self.0)))
    }
}

async fn run_handler(conn: HttpConn<Synthetic>, handler: Arc<impl Handler>) -> Conn {
    let conn = handler.run(conn.into()).await;
    handler.before_send(conn).await
}

async fn handler_fn(
    request: LambdaRequest,
    context: Context,
    handler: Arc<impl Handler>,
) -> std::io::Result<LambdaResponse> {
    match request {
        LambdaRequest::Alb(request) => {
            let mut conn = request.into_conn().await;
            conn.state_mut().insert(LambdaContext::new(context));
            let conn = run_handler(conn, handler).await;
            Ok(LambdaResponse::Alb(AlbResponse::from_conn(conn).await))
        }

        LambdaRequest::AlbMultiHeaders(request) => {
            let mut conn = request.into_conn().await;
            conn.state_mut().insert(LambdaContext::new(context));
            let conn = run_handler(conn, handler).await;
            Ok(LambdaResponse::AlbMultiHeaders(
                AlbMultiHeadersResponse::from_conn(conn).await,
            ))
        }
    }
}

pub async fn run_async(g: impl Handler) {
    lamedh_runtime::run(HandlerWrapper(Arc::new(g)))
        .await
        .unwrap()
}

pub fn run(g: impl Handler) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_async(g));
}
