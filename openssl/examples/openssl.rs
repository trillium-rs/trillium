use trillium::Conn;
use trillium_openssl::OpenSslAcceptor;

pub fn main() {
    env_logger::init();
    let cert = std::fs::read(std::env::var("CERT").expect("CERT env var")).unwrap();
    let key = std::fs::read(std::env::var("KEY").expect("KEY env var")).unwrap();

    trillium_smol::config()
        .with_acceptor(OpenSslAcceptor::from_single_cert(&cert, &key))
        .run(|conn: Conn| async move { conn.ok("ok") });
}
