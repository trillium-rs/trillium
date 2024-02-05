use async_io::Timer;
use std::time::Duration;
use trillium::Method;
use url::Url;

async fn handler((method, url, body): (Method, Url, String)) -> String {
    Timer::after(Duration::from_secs(5)).await;
    format!("received {method} to {url} with body {body}")
}

fn main() {
    env_logger::init();
    trillium_smol::run(trillium_api::cancel_on_disconnect(handler))
}
