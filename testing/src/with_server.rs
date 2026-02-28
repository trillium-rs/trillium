use crate::{ServerConnector, block_on, config};
use std::{error::Error, future::Future};
use trillium::Handler;
use trillium_http::transport::BoxedTransport;
use trillium_server_common::RuntimeTrait;
use url::Url;

/// Starts a trillium handler bound to a random available port on
/// localhost, run the async tests provided as the second
/// argument, and then shut down the server. useful for full
/// integration tests that actually exercise the tcp layer.
///
/// See
/// [`trillium_client::Conn`](https://docs.trillium.rs/trillium_client/struct.conn)
/// for usage examples.
pub fn with_server<H, Fun, Fut>(handler: H, tests: Fun)
where
    H: Handler,
    Fun: FnOnce(Url) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn Error>>>,
{
    let config = config().with_host("localhost").with_port(0);
    let runtime = config.runtime();
    runtime.block_on(async move {
        let handle = config.spawn(handler);
        let info = handle.info().await;
        let url = info.state().cloned().unwrap_or_else(|| {
            let port = info.tcp_socket_addr().map(|t| t.port()).unwrap_or(0);
            format!("http://localhost:{port}").parse().unwrap()
        });
        tests(url).await.unwrap();
        handle.shut_down().await;
    });
}

/// open an in-memory connection to this handler and call an async
/// function with an open BoxedTransport
pub fn with_transport<H, Fun, Fut>(handler: H, tests: Fun)
where
    H: Handler,
    Fun: FnOnce(BoxedTransport) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn Error>>>,
{
    block_on(async move {
        let transport = ServerConnector::new(handler).connect(false).await;
        tests(BoxedTransport::new(transport));
    });
}
