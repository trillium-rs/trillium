use trillium_logger::DevLogger;
type Proxy = trillium_proxy::Proxy<trillium_rustls::RustlsConnector<trillium_smol::TcpConnector>>;

pub fn main() {
    env_logger::init();
    trillium_smol::run((DevLogger, Proxy::new("https://httpbin.org/")));
}
