use async_channel::{unbounded, Receiver, Sender};
use async_tungstenite::{client_async, WebSocketStream};
use futures_util::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::pin::Pin;
use trillium::log_error;
use trillium_http::transport::BoxedTransport;
use trillium_websockets::{JsonWebSocketHandler, Message, Result, WebSocket, WebSocketConn};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct Response {
    inbound_message: Inbound,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct Inbound {
    message: String,
}

impl Inbound {
    fn new(message: &str) -> Self {
        Self {
            message: String::from(message),
        }
    }
}

struct SomeJsonChannel;
impl JsonWebSocketHandler for SomeJsonChannel {
    type InboundMessage = Inbound;
    type OutboundMessage = Response;
    type StreamType = Pin<Box<Receiver<Self::OutboundMessage>>>;

    async fn connect(&self, conn: &mut WebSocketConn) -> Self::StreamType {
        let (s, r) = unbounded();
        conn.set_state(s);
        Box::pin(r)
    }

    async fn receive_message(
        &self,
        inbound_message: Result<Self::InboundMessage>,
        conn: &mut WebSocketConn,
    ) {
        let Ok(inbound_message) = inbound_message else {
            return;
        };
        log_error!(
            conn.state::<Sender<Response>>()
                .unwrap()
                .send(Response { inbound_message })
                .await
        );
    }
}
async fn send<Out: Serialize, In: DeserializeOwned>(
    client: &mut WebSocketStream<BoxedTransport>,
    message: &Out,
) -> In {
    client
        .send(Message::text(serde_json::to_string(&message).unwrap()))
        .await
        .unwrap();

    serde_json::from_str(&client.next().await.unwrap().unwrap().into_text().unwrap()).unwrap()
}

#[test]
fn test() {
    trillium_testing::with_transport(
        WebSocket::new_json(SomeJsonChannel),
        |transport| async move {
            let (mut client, _) = client_async("ws://localhost/", transport).await?;

            let inbound_message = Inbound::new("hello");
            let response: Response = send(&mut client, &inbound_message).await;
            assert_eq!(response, Response { inbound_message });

            let inbound_message = Inbound::new("hey");
            let response: Response = send(&mut client, &inbound_message).await;
            assert_eq!(response, Response { inbound_message });

            Ok(())
        },
    );
}
