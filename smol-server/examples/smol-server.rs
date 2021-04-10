pub fn main() {
    env_logger::init();
    trillium_smol_server::run(|conn: trillium::Conn| async move { conn.ok("hello") });
}
