use myco::Conn;
use myco_native_tls::NativeTls;

pub fn main() {
    env_logger::init();
    myco_smol_server::run(
        "127.0.0.1:8000",
        NativeTls::from_pkcs12(include_bytes!("./identity.p12"), "changeit"),
        |conn: Conn| async move { dbg!(conn.ok("ok")) },
    );
}
