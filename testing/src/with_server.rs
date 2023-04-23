use crate::{block_on, ServerConnector};
use std::{error::Error, future::Future};
use trillium::Handler;
use trillium_http::transport::BoxedTransport;
use trillium_server_common::{Config, Connector, Server};
use url::Url;

/**
Starts a trillium handler bound to a random available port on
localhost, run the async tests provided as the second
argument, and then shut down the server. useful for full
integration tests that actually exercise the tcp layer.

See
[`trillium_client::Conn`](https://docs.trillium.rs/trillium_client/struct.conn)
for usage examples.
**/
pub fn with_server<H, Fun, Fut>(handler: H, tests: Fun)
where
    H: Handler,
    Fun: FnOnce(Url) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn Error>>>,
{
    block_on(async move {
        let port = portpicker::pick_unused_port().expect("could not pick a port");
        let url = format!("http://localhost:{port}").parse().unwrap();
        let handle = crate::config()
            .with_host("localhost")
            .with_port(port)
            .spawn(handler);
        handle.info().await;
        tests(url).await.unwrap();
        handle.stop().await;
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
