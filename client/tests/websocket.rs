use futures_lite::StreamExt;
use trillium_client::{websocket::Message, Client, WebSocketConn};
use trillium_testing::ClientConfig;
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

    let client = Client::new(ClientConfig::new());

    trillium_testing::with_server(handler, move |url| async move {
        let mut ws = client
            .get(url)
            .with_websocket_upgrade_headers()
            .await?
            .into_websocket()
            .await?;

        ws.send_string("Client test message".to_string()).await?;

        let response = ws.next().await.expect("response")?;

        assert_eq!(
            response,
            Message::Text("Server received your message: Client test message".to_string()),
        );

        Ok(())
    })
}
