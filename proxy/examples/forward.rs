use trillium_client::Client;
use trillium_logger::logger;
use trillium_proxy::{proxy, upstream::ForwardProxy, ForwardProxyConnect};
use trillium_smol::ClientConfig;

fn main() {
    trillium_smol::run((
        logger(),
        ForwardProxyConnect::new(ClientConfig::default()),
        proxy(
            Client::new(ClientConfig::default()).with_default_pool(),
            ForwardProxy,
        ),
    ));
}
