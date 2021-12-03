use broadcaster::BroadcastChannel;
use trillium_websockets::{Message, WebSocket, WebSocketConn, WebSocketHandler};

struct EchoServer {
    channel: BroadcastChannel<Message>,
}
impl EchoServer {
    fn new() -> Self {
        Self {
            channel: BroadcastChannel::new(),
        }
    }
}

#[trillium::async_trait]
impl WebSocketHandler for EchoServer {
    type OutboundStream = BroadcastChannel<Message>;

    async fn connect(&self, conn: WebSocketConn) -> Option<(WebSocketConn, Self::OutboundStream)> {
        Some((conn, self.channel.clone()))
    }

    async fn inbound(&self, message: Message, _conn: &mut WebSocketConn) {
        if let Message::Text(input) = message {
            let message = Message::text(format!("received message: {}", &input));
            trillium::log_error!(self.channel.send(&message).await);
        }
    }
}

fn main() {
    trillium_smol::run(WebSocket::new(EchoServer::new()));
}
