pub fn app() -> impl trillium::Handler {
    |conn: trillium::Conn| async move {
        let response = tokio::task::spawn(async {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            "successfully spawned a task"
        })
        .await
        .unwrap();
        conn.ok(response)
    }
}

#[tokio::main]
pub async fn main() {
    env_logger::init();
    let server = tokio::net::TcpListener::bind("localhost:8080")
        .await
        .unwrap();

    trillium_tokio::config()
        .with_prebound_server(server)
        .run_async(app())
        .await;
}
