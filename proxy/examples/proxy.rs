use async_net::TcpStream;
use myco_client::Rustls;

pub fn main() {
    env_logger::init();
    myco_smol_server::run(myco::sequence![
        myco_proxy::Proxy::<Rustls<TcpStream>>::new("https://httpbin.org/")
    ]);
}
