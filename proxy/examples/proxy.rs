use trillium_logger::Logger;
use trillium_proxy::Proxy;
use trillium_rustls::RustlsConfig;
use trillium_smol::ClientConfig;

pub fn main() {
    env_logger::init();
    let client_config = RustlsConfig::default().with_tcp_config(ClientConfig::default());
    trillium_smol::run((
        Logger::new(),
        Proxy::new(client_config, "https://httpbin.org/"),
    ));
}
