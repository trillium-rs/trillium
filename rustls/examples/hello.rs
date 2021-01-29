use std::io::BufReader;
use std::sync::Arc;

use async_tls::TlsAcceptor;
use myco::Conn;
use myco_rustls::Server;
use rustls::internal::pemfile::{certs, pkcs8_private_keys};
use rustls::{NoClientAuth, ServerConfig};

const KEY: &[u8] = include_bytes!("./key.pem");
const CERT: &[u8] = include_bytes!("./cert.pem");

pub fn main() {
    env_logger::init();
    let mut config = ServerConfig::new(NoClientAuth::new());

    config
        .set_single_cert(
            certs(&mut BufReader::new(CERT)).unwrap(),
            pkcs8_private_keys(&mut BufReader::new(KEY))
                .unwrap()
                .remove(0),
        )
        .unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(config));
    Server::new("127.0.0.1:8000", acceptor).run(|conn: Conn| async move { dbg!(conn.ok("ok")) });
}
