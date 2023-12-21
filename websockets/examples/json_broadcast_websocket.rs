use broadcaster::BroadcastChannel;
use std::net::IpAddr;
use trillium::{async_trait, Conn};
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

#[async_trait]
impl WebSocketHandler for EchoServer {
    type OutboundStream = BroadcastChannel<Message>;

    async fn connect(&self, conn: WebSocketConn) -> Option<(WebSocketConn, Self::OutboundStream)> {
        Some((conn, self.channel.clone()))
    }

    async fn inbound(&self, message: Message, conn: &mut WebSocketConn) {
        if let Message::Text(input) = message {
            let ip = conn
                .state()
                .map_or(String::from("<unknown>"), IpAddr::to_string);
            let message = Message::text(format!("received message `{}` from {}", input, ip));
            trillium::log_error!(self.channel.send(&message).await);
        }
    }
}

fn main() {
    env_logger::init();
    trillium_smol::run((
        trillium_logger::logger(),
        |mut conn: Conn| async move {
            if let Some(ip) = conn.peer_ip() {
                conn.set_state(ip);
            };
            conn
        },
        WebSocket::new(EchoServer::new()),
    ));
}
