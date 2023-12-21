use futures_lite::stream::{pending, Pending, Stream};
use futures_util::{SinkExt, StreamExt};
use std::pin::Pin;
use trillium::Handler;
use trillium_websockets::{Message, WebSocket, WebSocketConn, WebSocketHandler};

#[test]
fn with_handler_fn() {
    test_handler(WebSocket::new(|mut conn: WebSocketConn| async move {
        let path = conn.path().to_owned();
        while let Some(Ok(Message::Text(input))) = conn.next().await {
            conn.send_string(format!(
                "received your message: {} at path {}",
                &input, path
            ))
            .await
            .unwrap();
        }
    }));
}

#[test]
fn with_channel() {
    use async_channel::{Receiver, Sender};
    use trillium_websockets::{Message, WebSocket, WebSocketHandler};

    struct MyStruct;
    #[trillium::async_trait]
    impl WebSocketHandler for MyStruct {
        type OutboundStream = Pin<Box<Receiver<Message>>>;

        async fn connect(
            &self,
            mut conn: WebSocketConn,
        ) -> Option<(WebSocketConn, Self::OutboundStream)> {
            let (send, receive) = async_channel::unbounded();
            conn.set_state(send);
            Some((conn, Box::pin(receive)))
        }

        async fn inbound(&self, message: Message, conn: &mut WebSocketConn) {
            let path = conn.path().to_string();
            let sender: &mut Sender<Message> = conn.state_mut().unwrap();
            if let Message::Text(input) = message {
                let reply = Message::Text(format!(
                    "received your message: {} at path {}",
                    &input, &path
                ));

                trillium::log_error!(sender.send(reply).await);
            }
        }
    }

    test_handler(WebSocket::new(MyStruct));
}

#[test]
fn with_stream_only() {
    use async_channel::Sender;
    use trillium_websockets::{Message, WebSocket, WebSocketHandler};

    struct MyStruct;
    #[trillium::async_trait]
    impl WebSocketHandler for MyStruct {
        type OutboundStream = Pin<Box<dyn Stream<Item = Message> + Send + Sync + 'static>>;

        async fn connect(
            &self,
            mut conn: WebSocketConn,
        ) -> Option<(WebSocketConn, Self::OutboundStream)> {
            let path = conn.path().to_string();
            conn.take_inbound_stream().map(|stream| {
                (
                    conn,
                    Box::pin(stream.filter_map(move |message| {
                        let path = path.clone();
                        async move {
                            match message {
                                Ok(Message::Text(text)) => Some(Message::Text(format!(
                                    "received your message: {} at path {}",
                                    &text, &path
                                ))),
                                _ => None,
                            }
                        }
                    })) as Self::OutboundStream,
                )
            })
        }

        async fn inbound(&self, message: Message, conn: &mut WebSocketConn) {
            let path = conn.path().to_string();
            let sender: &mut Sender<Message> = conn.state_mut().unwrap();
            if let Message::Text(input) = message {
                let reply = Message::Text(format!(
                    "received your message: {} at path {}",
                    &input, &path
                ));

                trillium::log_error!(sender.send(reply).await);
            }
        }
    }

    test_handler(WebSocket::new(MyStruct));
}

#[test]
fn with_trait_directly() {
    struct MyStruct;

    #[trillium::async_trait]
    impl WebSocketHandler for MyStruct {
        type OutboundStream = Pending<Message>; // we don't use an outbound stream in this example

        async fn connect(
            &self,
            conn: WebSocketConn,
        ) -> Option<(WebSocketConn, Self::OutboundStream)> {
            Some((conn, pending()))
        }

        async fn inbound(&self, message: Message, conn: &mut WebSocketConn) {
            let path = conn.path().to_string();
            if let Message::Text(input) = message {
                let reply = format!("received your message: {} at path {}", &input, &path);
                conn.send_string(reply).await.unwrap();
            }
        }
    }

    test_handler(WebSocket::new(MyStruct));
}

fn test_handler(handler: impl Handler) {
    trillium_testing::with_transport(handler, |transport| async move {
        let (mut client, _) =
            async_tungstenite::client_async("ws://localhost/some/route", transport).await?;

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
