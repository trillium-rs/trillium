pub fn main() {
    trillium_smol::config()
        .with_port(1337)
        .with_host("127.0.0.1")
        .run((
            trillium_logger::Logger::new(),
            |conn: trillium::Conn| async move { conn.ok("hello world") },
        ))
}
