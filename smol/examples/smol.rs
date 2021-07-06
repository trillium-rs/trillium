pub fn main() {
    trillium_smol::run((
        trillium_logger::Logger::new(),
        |conn: trillium::Conn| async move { conn.ok("hello world") },
    ))
}
