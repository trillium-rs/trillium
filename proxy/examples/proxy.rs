use trillium_logger::Logger;
use trillium_rustls::RustlsConnector;
use trillium_smol::TcpConnector;

type Proxy = trillium_proxy::Proxy<RustlsConnector<TcpConnector>>;

pub fn main() {
    env_logger::init();
    trillium_smol::run((Logger::new(), Proxy::new("https://httpbin.org/")));
}
