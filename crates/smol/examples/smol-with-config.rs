pub fn main() {
    trillium_smol::config()
        .with_port(1337)
        .with_host("127.0.0.1")
        .run(|conn: trillium::Conn| async move { conn.ok("hello world") })
}
