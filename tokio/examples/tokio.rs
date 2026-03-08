pub fn app() -> impl trillium::Handler {
    |conn: trillium::Conn| async move { conn.ok("") }
}
pub fn main() {
    env_logger::init();
    trillium_tokio::run(app());
}
