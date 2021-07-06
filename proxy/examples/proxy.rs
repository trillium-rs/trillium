use trillium_logger::Logger;
use trillium_proxy::Proxy;
use trillium_rustls::{RustlsConfig, RustlsConnector};

pub fn main() {
    trillium_smol::run((
        Logger::new(),
        Proxy::new("https://httpbin.org/")
            .with_connector(RustlsConnector::new())
            .with_config(RustlsConfig::default()),
    ));
}
