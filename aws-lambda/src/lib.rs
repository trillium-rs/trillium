use lamedh_runtime::{Context, Handler};
use myco::{BoxedTransport, Conn, Grain};
use myco_http::{Conn as HttpConn, Synthetic};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

mod context;
mod request;
mod response;

use context::LambdaContext;
use request::LambdaRequest;
use response::{AlbMultiHeadersResponse, AlbResponse, LambdaResponse};

pub use context::LambdaConnExt;

struct GrainWrapper<G>(Arc<G>);

impl<G: Grain> Handler<LambdaRequest, LambdaResponse> for GrainWrapper<G> {
    type Error = myco::Error;
    type Fut = Pin<Box<dyn Future<Output = Result<LambdaResponse, Self::Error>> + Send + 'static>>;

    fn call(&mut self, request: LambdaRequest, context: Context) -> Self::Fut {
        Box::pin(grain_handler_fn(request, context, Arc::clone(&self.0)))
    }
}

async fn run_grain(conn: HttpConn<Synthetic>, grain: Arc<impl Grain>) -> Conn {
    let conn = Conn::new(conn.map_transport(BoxedTransport::new));
    let conn = grain.run(conn).await;
    grain.before_send(conn).await
}

async fn grain_handler_fn(
    request: LambdaRequest,
    context: Context,
    grain: Arc<impl Grain>,
) -> myco::Result<LambdaResponse> {
    match request {
        LambdaRequest::Alb(request) => {
            let mut conn = request.into_conn().await;
            conn.state_mut().insert(LambdaContext::new(context));
            let conn = run_grain(conn, grain).await;
            Ok(LambdaResponse::Alb(AlbResponse::from_conn(conn).await))
        }

        LambdaRequest::AlbMultiHeaders(request) => {
            let mut conn = request.into_conn().await;
            conn.state_mut().insert(LambdaContext::new(context));
            let conn = run_grain(conn, grain).await;
            Ok(LambdaResponse::AlbMultiHeaders(
                AlbMultiHeadersResponse::from_conn(conn).await,
            ))
        }
    }
}

pub async fn run_async(g: impl Grain) {
    lamedh_runtime::run(GrainWrapper(Arc::new(g)))
        .await
        .unwrap()
}

pub fn run(g: impl Grain) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_async(g));
}
