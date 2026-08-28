use futures_lite::StreamExt;
use trillium_client::{
    Client, WebSocketConn,
    websocket::{self, Message},
};
use trillium_http::Status;
use trillium_testing::client_config;
use trillium_websockets::websocket;

#[test]
fn test_websockets() {
    let handler = websocket(|mut conn: WebSocketConn| async move {
        while let Some(Ok(Message::Text(input))) = conn.next().await {
            conn.send_string(format!("Server received your message: {input}"))
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
            Message::text("Server received your message: Client test message"),
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

#[test]
fn path_and_querystring_round_trip() {
    let handler = websocket(|mut conn: WebSocketConn| async move {
        let reply = format!("path={} query={}", conn.path(), conn.querystring());
        while let Some(Ok(Message::Text(_))) = conn.next().await {
            conn.send_string(reply.clone()).await.expect("send_string");
        }
    });

    let client = Client::new(client_config());

    trillium_testing::with_server(handler, move |url| async move {
        let url = url.join("/some/route?foo=bar&baz")?;
        let mut ws = client.get(url).into_websocket().await?;

        assert_eq!(ws.path(), "/some/route");
        assert_eq!(ws.querystring(), "foo=bar&baz");

        ws.send_string("hello".to_string()).await?;
        assert_eq!(
            ws.next().await.expect("response")?,
            Message::text("path=/some/route query=foo=bar&baz")
        );

        Ok(())
    })
}
