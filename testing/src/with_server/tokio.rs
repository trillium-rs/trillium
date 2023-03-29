use crate::{block_on, Url};
use std::future::Future;
use trillium::Handler;
use trillium_server_common::Stopper;
use trillium_tokio::tokio::task::spawn;

/**
Starts a trillium handler bound to a random available port on
localhost, run the async tests provided as the second
argument, and then shut down the server. useful for full
integration tests that actually exercise the tcp layer.

See
[`trillium_client::Conn`](https://docs.trillium.rs/trillium_client/struct.conn)
for usage examples.
 */
pub fn with_server<H, Fun, Fut>(handler: H, tests: Fun)
where
    H: Handler,
    Fun: FnOnce(Url) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn std::error::Error>>>,
{
    block_on(async move {
        let port = portpicker::pick_unused_port().expect("could not pick a port");
        let url = format!("http://localhost:{}", port).parse().unwrap();
        let stopper = Stopper::new();
        let (s, r) = async_channel::bounded(1);
        let init = trillium::Init::new(move |_| async move {
            s.send(()).await.unwrap();
        });

        let server_future = spawn(
            trillium_tokio::config()
                .with_host("localhost")
                .with_port(port)
                .with_stopper(stopper.clone())
                .run_async((init, handler)),
        );
        r.recv().await.unwrap();
        let result = tests(url).await;
        stopper.stop();
        drop(server_future.await);
        result.unwrap()
    })
}

pub(crate) async fn tcp_connect(
    url: &Url,
) -> std::io::Result<trillium_http::transport::BoxedTransport> {
    Ok(trillium_http::transport::BoxedTransport::new(
        trillium_tokio::async_compat::Compat::new(
            trillium_tokio::tokio::net::TcpStream::connect(&url.socket_addrs(|| None)?[..]).await?,
        ),
    ))
}
