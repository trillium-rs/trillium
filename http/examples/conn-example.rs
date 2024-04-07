fn main() -> trillium_http::Result<()> {
    use async_net::{TcpListener, TcpStream};
    use futures_lite::StreamExt;
    use swansong::Swansong;
    use trillium_http::{Conn, Result};

    smol::block_on(async {
        let swansong = Swansong::new();

        let server_swansong = swansong.clone();
        let server = smol::spawn(async move {
            let listener = TcpListener::bind("localhost:8001").await?;
            let mut incoming = server_swansong.interrupt(listener.incoming());

            while let Some(Ok(stream)) = incoming.next().await {
                let swansong = server_swansong.clone();
                smol::spawn(async move {
                    Conn::map(stream, swansong, |mut conn: Conn<TcpStream>| async move {
                        conn.set_response_body("hello world");
                        conn.set_status(200);
                        conn
                    })
                    .await
                })
                .detach()
            }

            Result::Ok(())
        });

        // this example uses the trillium client
        // please note that this api is still especially unstable.
        // any other http client would work here too
        use trillium_client::Client;
        use trillium_smol::ClientConfig;
        let client = Client::new(ClientConfig::default());
        let mut client_conn = client.get("http://localhost:8001").await?;

        assert_eq!(client_conn.status().unwrap(), 200);
        assert_eq!(
            client_conn.response_body().read_string().await?,
            "hello world"
        );

        swansong.shut_down(); // stop the server after one request
        server.await?; // wait for the server to shut down

        Result::Ok(())
    })
}
