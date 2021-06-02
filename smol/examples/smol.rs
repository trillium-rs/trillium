pub fn main() {
    env_logger::init();
    trillium_smol::run(|conn: trillium::Conn| async move { conn.ok("hello") });
}
