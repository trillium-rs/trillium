use trillium::Conn;
use trillium_rustls::RustTls;

const KEY: &[u8] = include_bytes!("./key.pem");
const CERT: &[u8] = include_bytes!("./cert.pem");

pub fn main() {
    env_logger::init();
    trillium_smol_server::config()
        .with_acceptor(RustTls::from_pkcs8(CERT, KEY))
        .run(|conn: Conn| async move { conn.ok("ok") });
}
