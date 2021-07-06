use crate::Url;
use async_io::Timer;
use std::{future::Future, time::Duration};
use trillium::{Handler, Runtime};
use trillium_server_common::{Config, Server};

/**
Starts an trillium handler using the smol server bound to a random
available port on localhost, run the async tests provided as the
second argument, and then shut down the server. useful for full
integration tests that actually exercise the tcp layer.

See
[`trillium_client::Conn`](https://docs.trillium.rs/trillium_client/struct.conn)
for usage examples.

stability note: this doesn't really feel like it fits in the testing
crate, as it would not work well with a tokio-specific handler. it may
go away entirely at some point, or be moved to the trillium_smol crate
*/
pub fn with_server<S, H, Fun, Fut>(server: S, handler: H, tests: Fun)
where
    H: Handler<S>,
    Fun: Fn(Url) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn std::error::Error>>>,
    S: Server + Runtime + From<Config>,
{
    drop(server);

    S::block_on(async move {
        let port = portpicker::pick_unused_port().expect("could not pick a port");
        let url = format!("http://localhost:{}", port).parse().unwrap();
        let stopper = trillium_smol::Stopper::new();

        let config = Config::default()
            .with_host("localhost")
            .with_port(port)
            .with_stopper(stopper.clone());

        let handle = S::spawn_with_handle(S::from(config).run_async(handler));

        Timer::after(Duration::from_millis(500)).await;
        let result = tests(url).await;
        stopper.stop();
        handle.await;
        result.unwrap()
    })
}
