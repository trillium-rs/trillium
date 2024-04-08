#![forbid(unsafe_code)]
#![deny(
    clippy::dbg_macro,
    missing_copy_implementations,
    rustdoc::missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

//! # Trillium server adapter for aws lambda
//!
//! ```rust,no_run
//! trillium_aws_lambda::run(|conn: trillium::Conn| async move { conn.ok("hello lambda") });
//! ```

use lamedh_runtime::{Context, Handler as AwsHandler};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::runtime;
use trillium::{Conn, Handler};
use trillium_http::{Conn as HttpConn, ServerConfig, Synthetic};

mod context;
pub use context::LambdaConnExt;
use context::LambdaContext;

mod request;
use request::LambdaRequest;

mod response;
use response::{AlbMultiHeadersResponse, AlbResponse, LambdaResponse};

#[derive(Debug)]
struct HandlerWrapper<H>(Arc<H>, Arc<ServerConfig>);

impl<H: Handler> AwsHandler<LambdaRequest, LambdaResponse> for HandlerWrapper<H> {
    type Error = std::io::Error;
    type Fut = Pin<Box<dyn Future<Output = Result<LambdaResponse, Self::Error>> + Send + 'static>>;

    fn call(&mut self, request: LambdaRequest, context: Context) -> Self::Fut {
        Box::pin(handler_fn(
            request,
            context,
            Arc::clone(&self.0),
            Arc::clone(&self.1),
        ))
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
    server_config: Arc<ServerConfig>,
) -> std::io::Result<LambdaResponse> {
    match request {
        LambdaRequest::Alb(request) => {
            let mut conn = request.into_conn().await.with_server_config(server_config);
            conn.state_mut().insert(LambdaContext::new(context));
            let conn = run_handler(conn, handler).await;
            Ok(LambdaResponse::Alb(AlbResponse::from_conn(conn).await))
        }

        LambdaRequest::AlbMultiHeaders(request) => {
            let mut conn = request.into_conn().await.with_server_config(server_config);
            conn.state_mut().insert(LambdaContext::new(context));
            let conn = run_handler(conn, handler).await;
            Ok(LambdaResponse::AlbMultiHeaders(
                AlbMultiHeadersResponse::from_conn(conn).await,
            ))
        }
    }
}
/// # Runs a trillium handler on an already-running tokio runtime
///
/// This function will poll pending until the server shuts down.
pub async fn run_async(mut handler: impl Handler) {
    let mut info = ServerConfig::default().into();
    handler.init(&mut info).await;
    lamedh_runtime::run(HandlerWrapper(Arc::new(handler), Arc::new(info.into())))
        .await
        .unwrap()
}

/// # Runs a trillium handler in a sync context
///
/// This function creates a new tokio runtime and executes the handler on
/// it for aws lambda.
///
/// This function will block the current thread until the server shuts
/// down

pub fn run(handler: impl Handler) {
    runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_async(handler));
}
