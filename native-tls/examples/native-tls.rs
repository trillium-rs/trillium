use trillium::Conn;
use trillium_native_tls::NativeTls;

pub fn main() {
    env_logger::init();
    trillium_smol_server::config()
        .with_acceptor(NativeTls::from_pkcs12(
            include_bytes!("./identity.p12"),
            "changeit",
        ))
        .run(|conn: Conn| async move { conn.ok("ok") });
}
