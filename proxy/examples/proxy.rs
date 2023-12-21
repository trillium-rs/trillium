use trillium_logger::Logger;
use trillium_proxy::Proxy;
use trillium_smol::ClientConfig;

pub fn main() {
    env_logger::init();
    let client_config = ClientConfig::default();
    trillium_smol::run((
        Logger::new(),
        Proxy::new(client_config, "http://localhost:8081/").with_via_pseudonym("trillium-proxy"),
    ));
}
