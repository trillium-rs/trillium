use async_channel::{unbounded, Receiver, Sender};
use serde::{Deserialize, Serialize};
use trillium::{async_trait, log_error};
use trillium_websockets::{json_websocket, JsonWebSocketHandler, WebSocketConn};

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct Response {
    inbound_message: Inbound,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
struct Inbound {
    message: String,
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

fn main() {
    trillium_smol::run(json_websocket(SomeJsonChannel));
}
