use async_tungstenite::client_async;
use futures_lite::{AsyncReadExt, AsyncWriteExt, StreamExt};
use trillium::Handler;
use trillium_websockets::{
    Message, WebSocketConn, WebSocketHandler,
    tungstenite::protocol::{
        CloseFrame,
        frame::{
            Frame,
            coding::{CloseCode, Data, OpCode},
        },
    },
    websocket,
};

/// Echoes text and binary messages and polls until the stream ends, which is what writes the reply
/// to a client-initiated close.
fn echo() -> impl Handler {
    websocket(|mut conn: WebSocketConn| async move {
        while let Some(Ok(message)) = conn.next().await {
            if (message.is_text() || message.is_binary()) && conn.send(message).await.is_err() {
                break;
            }
        }
    })
}

#[test]
fn large_messages_round_trip() {
    trillium_testing::with_transport(echo(), |transport| async move {
        let (mut client, _) = client_async("ws://localhost/", transport).await?;

        let binary: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
        client.send(Message::binary(binary.clone())).await?;
        assert_eq!(client.next().await.unwrap()?, Message::binary(binary));

        let text = "trillium ".repeat(20_000);
        client.send(Message::text(text.clone())).await?;
        assert_eq!(client.next().await.unwrap()?, Message::text(text));

        Ok(())
    });
}

#[test]
fn fragmented_message_is_reassembled() {
    trillium_testing::with_transport(echo(), |transport| async move {
        let (mut client, _) = client_async("ws://localhost/", transport).await?;

        for frame in [
            Frame::message("hello ", OpCode::Data(Data::Text), false),
            Frame::message("fragmented ", OpCode::Data(Data::Continue), false),
            Frame::message("world", OpCode::Data(Data::Continue), true),
        ] {
            client.send(Message::Frame(frame)).await?;
        }

        assert_eq!(
            client.next().await.unwrap()?,
            Message::text("hello fragmented world")
        );
        Ok(())
    });
}

#[test]
fn ping_is_answered_without_handler_involvement() {
    trillium_testing::with_transport(echo(), |transport| async move {
        let (mut client, _) = client_async("ws://localhost/", transport).await?;
        client.send(Message::Ping("are you there".into())).await?;
        assert_eq!(
            client.next().await.unwrap()?,
            Message::Pong("are you there".into())
        );
        Ok(())
    });
}

#[test]
fn client_initiated_close_is_answered() {
    trillium_testing::with_transport(echo(), |transport| async move {
        let (mut client, _) = client_async("ws://localhost/", transport).await?;
        let close = CloseFrame {
            code: CloseCode::Normal,
            reason: "done".into(),
        };
        client.close(Some(close.clone())).await?;
        assert_eq!(client.next().await.unwrap()?, Message::Close(Some(close)));
        assert!(client.next().await.is_none());
        Ok(())
    });
}

#[test]
fn client_initiated_close_is_answered_by_a_websocket_handler() {
    struct Echo;
    impl WebSocketHandler for Echo {
        type OutboundStream = futures_lite::stream::Pending<Message>;

        async fn connect(
            &self,
            conn: WebSocketConn,
        ) -> Option<(WebSocketConn, Self::OutboundStream)> {
            Some((conn, futures_lite::stream::pending()))
        }

        async fn inbound(&self, message: Message, conn: &mut WebSocketConn) {
            conn.send(message).await.unwrap();
        }
    }

    trillium_testing::with_transport(websocket(Echo), |transport| async move {
        let (mut client, _) = client_async("ws://localhost/", transport).await?;
        client.send(Message::text("hello")).await?;
        assert_eq!(client.next().await.unwrap()?, Message::text("hello"));

        client.close(None).await?;
        assert_eq!(client.next().await.unwrap()?, Message::Close(None));
        assert!(client.next().await.is_none());
        Ok(())
    });
}

#[test]
fn server_initiated_close() {
    let handler = websocket(|mut conn: WebSocketConn| async move {
        conn.next().await;
        conn.close().await.unwrap();
        // the close handshake completes when the client's reply arrives
        while conn.next().await.is_some() {}
    });

    trillium_testing::with_transport(handler, |transport| async move {
        let (mut client, _) = client_async("ws://localhost/", transport).await?;
        client.send(Message::text("goodbye")).await?;
        assert_eq!(client.next().await.unwrap()?, Message::Close(None));
        assert!(client.next().await.is_none());
        Ok(())
    });
}

#[test]
fn frame_sent_with_the_handshake_is_received() {
    trillium_testing::with_transport(echo(), |mut transport| async move {
        let mask = [1, 2, 3, 4];
        let mut request = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: \
                            websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: \
                            dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
            .to_vec();
        request.extend([0x81, 0x80 | 2]);
        request.extend(mask);
        request.extend(b"hi".iter().zip(mask.iter().cycle()).map(|(b, m)| b ^ m));
        transport.write_all(&request).await?;

        let echoed = [0x81, 2, b'h', b'i'];
        let mut received = Vec::new();
        let mut buf = [0; 1024];
        while !received.ends_with(&echoed) {
            let len = transport.read(&mut buf).await?;
            assert_ne!(len, 0, "eof before the echo: {received:?}");
            received.extend_from_slice(&buf[..len]);
        }
        assert!(received.starts_with(b"HTTP/1.1 101"));
        Ok(())
    });
}

#[test]
fn fed_messages_beyond_the_write_buffer_all_arrive() {
    const COUNT: usize = 50;
    const LEN: usize = 64 * 1024;

    let handler = websocket(|mut conn: WebSocketConn| async move {
        conn.next().await;
        for i in 0..COUNT {
            conn.feed(Message::binary(vec![i as u8; LEN]))
                .await
                .unwrap();
        }
        conn.flush().await.unwrap();
        while conn.next().await.is_some() {}
    });

    trillium_testing::with_transport(handler, |transport| async move {
        let (mut client, _) = client_async("ws://localhost/", transport).await?;
        client.send(Message::text("go")).await?;
        for i in 0..COUNT {
            assert_eq!(
                client.next().await.unwrap()?,
                Message::binary(vec![i as u8; LEN])
            );
        }
        Ok(())
    });
}
