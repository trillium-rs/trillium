use futures_lite::StreamExt;
use serde::{Deserialize, Serialize};
use trillium::{async_trait, state, Conn};
use trillium_caching_headers::caching_headers;
use trillium_channels::{channel, ChannelBroadcaster, ChannelConn, ChannelEvent, ChannelHandler};
use trillium_conn_id::{conn_id, log_formatter};
use trillium_logger::{apache_common, logger};
use trillium_router::router;
use trillium_static_compiled::static_compiled;

struct ChatChannel;
#[async_trait]
impl ChannelHandler for ChatChannel {
    async fn join_channel(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
        if event.topic() == "rooms:lobby" {
            conn.allow_join(&event, &()).await;
            conn.broadcast(("rooms:lobby", "user:entered"));
        }
    }

    async fn incoming_message(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
        if event.topic() == "rooms:lobby" && event.event() == "new:msg" {
            conn.broadcast(event);
        }
    }
}

fn spawn_logger(mut broadcast_stream: ChannelBroadcaster) {
    trillium_smol::async_global_executor::spawn(async move {
        while let Some(event) = broadcast_stream.next().await {
            if event.payload().is_null() {
                println!("[{}] {}", event.topic(), event.event());
            } else {
                println!(
                    "[{}] {} {}",
                    event.topic(),
                    event.event(),
                    serde_json::to_string(event.payload()).unwrap()
                );
            }
        }
    })
    .detach();
}

fn main() {
    let channels = channel(ChatChannel);
    let broadcast = channels.broadcaster();
    spawn_logger(broadcast.clone());

    trillium_smol::run((
        conn_id(),
        logger().with_formatter(apache_common(log_formatter::conn_id, "-")),
        caching_headers(),
        static_compiled!("examples/files").with_index_file("index.html"),
        router().get("/socket/websocket", channels).put(
            "/broadcast",
            (
                state(broadcast),
                trillium_api::api(broadcast_from_elsewhere),
            ),
        ),
    ))
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    body: String,
    user: Option<String>,
}

async fn broadcast_from_elsewhere(conn: Conn, message: ChatMessage) -> Conn {
    let sender = conn.state::<ChannelBroadcaster>().unwrap();
    sender.broadcast(("rooms:lobby", "new:msg", message));
    let response = format!("ok, clients: {}", sender.connected_clients());
    conn.ok(response)
}
