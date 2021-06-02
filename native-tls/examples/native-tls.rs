use trillium::Conn;
use trillium_native_tls::NativeTlsAcceptor;

pub fn main() {
    env_logger::init();
    let acceptor = NativeTlsAcceptor::from_pkcs12(include_bytes!("./identity.p12"), "changeit");
    trillium_smol::config()
        .with_acceptor(acceptor)
        .run(|conn: Conn| async move { conn.ok("ok") });
}
