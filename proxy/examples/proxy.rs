use async_net::TcpStream;
use trillium_client::Rustls;

pub fn main() {
    env_logger::init();
    trillium_smol_server::run(trillium::sequence![
        trillium_proxy::Proxy::<Rustls<TcpStream>>::new("https://httpbin.org/")
    ]);
}
