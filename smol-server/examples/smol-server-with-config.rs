pub fn main() {
    myco_smol_server::config()
        .with_port(1337)
        .with_host("127.0.0.1")
        .run(|conn: myco::Conn| async move { conn.ok("hello world") })
}
