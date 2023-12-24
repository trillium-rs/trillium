pub fn app() -> impl trillium::Handler {
    |conn: trillium::Conn| async move {
        tokio::task::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            panic!()
        })
        .await
        .unwrap();

        conn.ok("")
    }
}
pub fn main() {
    env_logger::init();
    trillium_tokio::run(app());
}
