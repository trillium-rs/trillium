use trillium_client::Client;
use trillium_logger::Logger;
use trillium_proxy::{
    upstream::{ConnectionCounting, IntoUpstreamSelector, UpstreamSelector},
    Proxy,
};
use trillium_smol::ClientConfig;

pub fn main() {
    env_logger::init();
    let upstream = if std::env::args().count() == 1 {
        "http://localhost:8080".into_upstream().boxed()
    } else {
        std::env::args()
            .into_iter()
            .skip(1)
            .collect::<ConnectionCounting<_>>()
            .boxed()
    };

    trillium_smol::run((
        Logger::new(),
        Proxy::new(
            Client::new(ClientConfig::default()).with_default_pool(),
            upstream,
        )
        .with_via_pseudonym("trillium-proxy"),
    ));
}
