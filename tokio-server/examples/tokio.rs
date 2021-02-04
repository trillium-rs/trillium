pub fn main() {
    env_logger::init();
    myco_tokio_server::run("localhost:8000", (), |conn: myco::Conn| async move {
        dbg!(conn)
    });
}
