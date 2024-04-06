use async_channel::{unbounded, Receiver, Sender};
use serde::{Deserialize, Serialize};
use trillium_websockets::{json_websocket, JsonWebSocketHandler, Result, WebSocketConn};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
enum Response {
    Ack(Inbound),
    ParseError(String),
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct Inbound {
    message: String,
}

struct SomeJsonChannel;

impl JsonWebSocketHandler for SomeJsonChannel {
    type InboundMessage = Inbound;
    type OutboundMessage = Response;
    type StreamType = Receiver<Self::OutboundMessage>;

    async fn connect(&self, conn: &mut WebSocketConn) -> Self::StreamType {
        let (s, r) = unbounded();
        conn.insert_state(s);
        r
    }

    async fn receive_message(
        &self,
        inbound_message: Result<Self::InboundMessage>,
        conn: &mut WebSocketConn,
    ) {
        let response = match inbound_message {
            Ok(message) => Response::Ack(message),
            Err(e) => Response::ParseError(e.to_string()),
        };

        if let Err(e) = conn
            .state::<Sender<Response>>()
            .unwrap()
            .send(response)
            .await
        {
            log::error!("send error: {e}");
        }
    }
}

fn main() {
    trillium_smol::run(json_websocket(SomeJsonChannel));
}
