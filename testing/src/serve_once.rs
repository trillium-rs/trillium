use crate::Url;
use async_io::Timer;
use std::{future::Future, time::Duration};
use trillium::Handler;

pub fn serve_once<H, Fun, Fut>(handler: H, tests: Fun)
where
    H: Handler,
    Fun: Fn(Url) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn std::error::Error>>>,
{
    async_global_executor::block_on(async move {
        let port = portpicker::pick_unused_port().expect("could not pick a port");
        let url = format!("http://localhost:{}", port).parse().unwrap();
        let stopper = trillium_smol::Stopper::new();

        let server_future = async_global_executor::spawn(
            trillium_smol::config()
                .with_host("localhost")
                .with_port(port)
                .with_stopper(stopper.clone())
                .run_async(handler),
        );

        Timer::after(Duration::from_millis(500)).await;
        let result = tests(url).await;
        stopper.stop();
        server_future.await;
        result.unwrap()
    })
}
