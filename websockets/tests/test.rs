#[test]
fn smoke() {
    use futures_util::{SinkExt, StreamExt};
    use trillium_websockets::{Message, WebSocket};

    let handler = WebSocket::new(|mut websocket| async move {
        let path = websocket.path().to_owned();
        while let Some(Ok(Message::Text(input))) = websocket.next().await {
            websocket
                .send_string(format!(
                    "received your message: {} at path {}",
                    &input, path
                ))
                .await;
        }
    });

    trillium_testing::with_socket(handler, |socket| async move {
        let (mut client, _) =
            async_tungstenite::client_async("ws://localhost/some/route", socket).await?;

        client.send(Message::text("hello")).await?;
        let received_message = client.next().await.unwrap()?.into_text()?;
        assert_eq!(
            "received your message: hello at path /some/route",
            received_message
        );

        client.send(Message::text("hey")).await?;
        let received_message = client.next().await.unwrap()?.into_text()?;
        assert_eq!(
            "received your message: hey at path /some/route",
            received_message
        );

        Ok(())
    });
}
