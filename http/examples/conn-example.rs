fn main() {
    use smol::{net::TcpListener, stream::StreamExt};
    use std::sync::Arc;
    use trillium_http::ServerConfig;

    smol::block_on(async {
        let server_config = Arc::new(ServerConfig::default());
        let listener = TcpListener::bind("localhost:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();
        println!("listening on http://{local_addr}");

        let server = smol::spawn({
            let server_config = server_config.clone();
            async move {
                let mut incoming = server_config.swansong().interrupt(listener.incoming());

                while let Some(Ok(stream)) = incoming.next().await {
                    smol::spawn(server_config.clone().run(stream, |mut conn| async move {
                        conn.set_response_body("hello world");
                        conn.set_status(200);
                        conn
                    }))
                    .detach()
                }
            }
        });

        use trillium_client::Client;
        use trillium_smol::ClientConfig;
        let client = Client::new(ClientConfig::default()).with_base(local_addr);
        let mut client_conn = client.get("/").await.unwrap();

        assert_eq!(client_conn.status().unwrap(), 200);
        assert_eq!(
            client_conn.response_body().read_string().await.unwrap(),
            "hello world"
        );

        server.await;

        // server_config.shut_down().await; // stop the server after one request
    })
}
