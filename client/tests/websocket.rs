use futures_lite::StreamExt;
use trillium_client::{
    websocket::{self, Message},
    Client, WebSocketConn,
};
use trillium_http::Status;
use trillium_testing::client_config;
use trillium_websockets::websocket;

#[test]
fn test_websockets() {
    let handler = websocket(|mut conn: WebSocketConn| async move {
        while let Some(Ok(Message::Text(input))) = conn.next().await {
            conn.send_string(format!("Server received your message: {}", &input))
                .await
                .expect("send_string");
        }
    });

    let client = Client::new(client_config());

    trillium_testing::with_server(handler, move |url| async move {
        let mut ws = client.get(url).into_websocket().await?;

        ws.send_string("Client test message".to_string()).await?;

        let response = ws.next().await.expect("response")?;

        assert_eq!(
            response,
            Message::Text("Server received your message: Client test message".to_string()),
        );

        Ok(())
    })
}

#[test]
fn test_websockets_error() {
    let handler =
        |conn: trillium::Conn| async { conn.with_status(404).with_body("This does not exist") };
    let client = Client::new(client_config());
    trillium_testing::with_server(handler, move |url| async move {
        let err = client
            .get(url)
            .into_websocket()
            .await
            .expect_err("Expected a 404");
        assert!(matches!(
            err.kind,
            websocket::ErrorKind::Status(Status::NotFound),
        ));
        let mut conn = trillium_client::Conn::from(err);
        let body = conn.response_body().read_string().await?;
        assert_eq!(body, "This does not exist");

        Ok(())
    })
}
