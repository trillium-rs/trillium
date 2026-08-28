use futures_lite::stream::{Pending, Stream, StreamExt, pending};
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
    impl WebSocketHandler for MyStruct {
        type OutboundStream = Pin<Box<Receiver<Message>>>;

        async fn connect(
            &self,
            mut conn: WebSocketConn,
        ) -> Option<(WebSocketConn, Self::OutboundStream)> {
            let (send, receive) = async_channel::unbounded();
            conn.insert_state(send);
            Some((conn, Box::pin(receive)))
        }

        async fn inbound(&self, message: Message, conn: &mut WebSocketConn) {
            let path = conn.path().to_string();
            let sender: &mut Sender<Message> = conn.state_mut().unwrap();
            if let Message::Text(input) = message {
                let reply = Message::text(format!(
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
                    Box::pin(
                        stream
                            .then(move |message| {
                                let path = path.clone();
                                async move {
                                    match message {
                                        Ok(Message::Text(text)) => Some(Message::text(format!(
                                            "received your message: {} at path {}",
                                            &text, &path
                                        ))),
                                        _ => None,
                                    }
                                }
                            })
                            .filter_map(|x| x),
                    ) as Self::OutboundStream,
                )
            })
        }

        async fn inbound(&self, message: Message, conn: &mut WebSocketConn) {
            let path = conn.path().to_string();
            let sender: &mut Sender<Message> = conn.state_mut().unwrap();
            if let Message::Text(input) = message {
                let reply = Message::text(format!(
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

    impl WebSocketHandler for MyStruct {
        type OutboundStream = Pending<Message>;

        // we don't use an outbound stream in this example

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
        let received_message = client.next().await.unwrap()?;
        assert_eq!(
            "received your message: hello at path /some/route",
            received_message.to_string()
        );

        client.send(Message::text("hey")).await?;
        let received_message = client.next().await.unwrap()?.into_text()?;
        assert_eq!(
            "received your message: hey at path /some/route",
            received_message.to_string()
        );

        Ok(())
    });
}

fn echo_request_target() -> WebSocket<impl WebSocketHandler> {
    WebSocket::new(|mut conn: WebSocketConn| async move {
        let reply = format!("path={} query={}", conn.path(), conn.querystring());
        while let Some(Ok(Message::Text(_))) = conn.next().await {
            conn.send_string(reply.clone()).await.unwrap();
        }
    })
}

fn assert_request_target(handler: impl Handler, url: &'static str, expected: &'static str) {
    trillium_testing::with_transport(handler, move |transport| async move {
        let (mut client, _) = async_tungstenite::client_async(url, transport).await?;
        client.send(Message::text("hello")).await?;
        assert_eq!(expected, client.next().await.unwrap()?.to_string());
        Ok(())
    });
}

#[test]
fn path_and_querystring_are_populated_independently() {
    assert_request_target(
        echo_request_target(),
        "ws://localhost/some/route?foo=bar&baz",
        "path=/some/route query=foo=bar&baz",
    );
}

#[test]
fn querystring_is_empty_when_absent() {
    assert_request_target(
        echo_request_target(),
        "ws://localhost/some/route",
        "path=/some/route query=",
    );
}

/// The upgrade-dispatch analog of a nested router mount, without depending on `trillium-router`:
/// all a router does to the path is push the wildcard remainder onto the upgrade.
struct MountedAt<H>(&'static str, H);

impl<H: Handler> Handler for MountedAt<H> {
    async fn run(&self, conn: trillium::Conn) -> trillium::Conn {
        self.1.run(conn).await
    }

    fn has_upgrade(&self, upgrade: &trillium::Upgrade) -> bool {
        upgrade.push_path(String::from(self.0));
        let has_upgrade = self.1.has_upgrade(upgrade);
        upgrade.pop_path();
        has_upgrade
    }

    async fn upgrade(&self, upgrade: trillium::Upgrade) {
        upgrade.push_path(String::from(self.0));
        self.1.upgrade(upgrade).await;
    }
}

#[test]
fn a_pushed_path_frame_narrows_the_path_but_not_the_querystring() {
    assert_request_target(
        MountedAt("route", echo_request_target()),
        "ws://localhost/some/route?foo=bar&baz",
        "path=route query=foo=bar&baz",
    );
}
