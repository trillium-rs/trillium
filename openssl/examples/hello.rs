use myco::Conn;
use myco_openssl::Server;

const KEY: &[u8] = include_bytes!("./identity.p12");

pub fn main() {
    env_logger::init();
    Server::new("127.0.0.1:8000", KEY.into(), "changeit".into())
        .run(|conn: Conn| async move { dbg!(conn.ok("ok")) });
}
