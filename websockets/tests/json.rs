use async_channel::{unbounded, Receiver, Sender};
use async_tungstenite::{client_async, WebSocketStream};
use futures_util::{SinkExt, StreamExt};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::error::Error;
use trillium::{async_trait, log_error};
use trillium_http::transport::BoxedTransport;
use trillium_websockets::{JsonWebSocketHandler, Message, WebSocket, WebSocketConn};

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
#[async_trait]
impl JsonWebSocketHandler for SomeJsonChannel {
    type InboundMessage = Inbound;
    type OutboundMessage = Response;
    type StreamType = Receiver<Self::OutboundMessage>;

    async fn connect(&self, conn: &mut WebSocketConn) -> Self::StreamType {
        let (s, r) = unbounded();
        conn.set_state(s);
        r
    }

    async fn receive_message(
        &self,
        inbound_message: Self::InboundMessage,
        conn: &mut WebSocketConn,
    ) {
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
) -> Result<In, Box<dyn Error>> {
    client
        .send(Message::text(serde_json::to_string(&message)?))
        .await?;

    Ok(serde_json::from_str(
        &client.next().await.ok_or("stream closed")??.into_text()?,
    )?)
}

#[test]
fn test() {
    trillium_testing::with_transport(
        WebSocket::new_json(SomeJsonChannel),
        |transport| async move {
            let (mut client, _) = client_async("ws://localhost/", transport).await?;

            let inbound_message = Inbound::new("hello");
            let response: Response = send(&mut client, &inbound_message).await?;
            assert_eq!(response, Response { inbound_message });

            let inbound_message = Inbound::new("hey");
            let response: Response = send(&mut client, &inbound_message).await?;
            assert_eq!(response, Response { inbound_message });

            Ok(())
        },
    );
}
