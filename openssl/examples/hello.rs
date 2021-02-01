use myco::Conn;
use native_tls::{Identity, TlsAcceptor};

const KEY: &[u8] = include_bytes!("./identity.p12");

pub fn main() {
    env_logger::init();
    let acceptor = TlsAcceptor::new(Identity::from_pkcs12(KEY, "changeit").unwrap()).unwrap();
    myco_openssl::run("127.0.0.1:8000", acceptor, |conn: Conn| async move {
        dbg!(conn.ok("ok"))
    });
}
