use myco::Conn;
use myco_rustls::RustTls;

const KEY: &[u8] = include_bytes!("./key.pem");
const CERT: &[u8] = include_bytes!("./cert.pem");

pub fn main() {
    env_logger::init();
    myco_smol_server::run(
        "localhost:8000",
        RustTls::from_pkcs8(CERT, KEY),
        |conn: Conn| async move { dbg!(conn.ok("ok")) },
    );
}
