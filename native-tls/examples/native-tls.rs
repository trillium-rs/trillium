use trillium::Conn;
use trillium_native_tls::NativeTlsAcceptor;

pub fn main() {
    env_logger::init();
    let acceptor = NativeTlsAcceptor::from_cert_and_key(
        include_bytes!("../tests/fixtures/rsa.crt"),
        include_bytes!("../tests/fixtures/rsa-pkcs8.key"),
    );
    trillium_smol::config()
        .with_acceptor(acceptor)
        .run(|conn: Conn| async move { conn.ok("ok") });
}
