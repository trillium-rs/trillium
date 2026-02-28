use trillium_channels::{ChannelConn, ChannelEvent, ChannelHandler, channel};

struct ChatChannel;
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

fn main() {
    trillium_smol::run(channel(ChatChannel));
}
