pub fn main() {
    env_logger::init();
    myco_tokio_server::run(|conn: myco::Conn| async move { conn.ok("ok!") });
}
