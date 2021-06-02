fn main() -> trillium_http::Result<()> {
    use async_net::{TcpListener, TcpStream};
    use futures_lite::StreamExt;
    use stopper::Stopper;
    use trillium_http::{Conn, Result};

    type ClientConn<'a> = trillium_client::Conn<'a, trillium_smol::TcpConnector>;

    smol::block_on(async {
        let stopper = Stopper::new();

        let server_stopper = stopper.clone();
        let server = smol::spawn(async move {
            let listener = TcpListener::bind("localhost:8001").await?;
            let mut incoming = server_stopper.stop_stream(listener.incoming());

            while let Some(Ok(stream)) = incoming.next().await {
                let stopper = server_stopper.clone();
                smol::spawn(async move {
                    Conn::map(stream, stopper, |mut conn: Conn<TcpStream>| async move {
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
        let mut client_conn = ClientConn::get("http://localhost:8001").execute().await?;

        assert_eq!(client_conn.status().unwrap(), 200);
        assert_eq!(
            client_conn.response_body().read_string().await?,
            "hello world"
        );

        stopper.stop(); // stop the server after one request
        server.await?; // wait for the server to shut down

        Result::Ok(())
    })
}
