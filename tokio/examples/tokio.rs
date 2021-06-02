pub fn main() {
    env_logger::init();
    trillium_tokio::run(|conn: trillium::Conn| async move { conn.ok("ok!") });
}
