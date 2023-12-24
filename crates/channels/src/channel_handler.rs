use crate::{ChannelConn, ChannelEvent};

/**
# Trait for you to implement in order to define a [`Channel`](crate::Channel).

## Example

This simple example represents a simple chat server that's
compatible with the [phoenix chat
example](https://github.com/chrismccord/phoenix_chat_example) -- see
channels/examples/channels.rs in this repo for a runnable example.

The only behavior we need to implement:

* allow users to join the lobby channel
* broadcast to all users when a new user has joined the lobby
* broadcast all messages sent to the lobby channel to all users
  subscribed to the lobby channel.

```
use trillium_channels::{channel, ChannelConn, ChannelEvent, ChannelHandler};

struct ChatChannel;
#[trillium::async_trait]
impl ChannelHandler for ChatChannel {
    async fn join_channel(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
        match event.topic() {
            "rooms:lobby" => {
                conn.allow_join(&event, &()).await;
                conn.broadcast(("rooms:lobby", "user:entered"));
            }

            _ => {}
        }
    }

    async fn incoming_message(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
        match (event.topic(), event.event()) {
            ("rooms:lobby", "new:msg") => conn.broadcast(event),
            _ => {}
        }
    }
}

// fn main() {
//     trillium_smol::run(channel(ChatChannel));
// }
```

*/
#[allow(unused_variables)]
#[trillium::async_trait]
pub trait ChannelHandler: Sized + Send + Sync + 'static {
    /**
    `connect` is called once when each websocket client is connected. The default implementation does nothing.
     */
    async fn connect(&self, conn: ChannelConn<'_>) {}

    /**
    `join_channel` is called when a websocket client sends a
    `phx_join` event. There is no default implementation to ensure
    that you implement the appropriate access control logic for your
    application. If you want clients to be able to connect to any
    channel they request, use this definition:

    ```
    # use trillium_channels::{ChannelEvent, ChannelConn, ChannelHandler};
    # struct MyChannel; #[trillium::async_trait] impl ChannelHandler for MyChannel {
    async fn join_channel(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
        conn.allow_join(&event, &()).await;
    }
    # }
    ```
    */
    async fn join_channel(&self, conn: ChannelConn<'_>, event: ChannelEvent);

    /**
    `leave_channel` is called when a websocket client sends a
    `phx_leave` event. The default implementation is to allow the user
    to leave that channel.
    */
    async fn leave_channel(&self, conn: ChannelConn<'_>, event: ChannelEvent) {
        conn.allow_leave(&event, &()).await
    }

    /**
    `incoming_message` is called once for each [`ChannelEvent`] sent
    from a client. The default implementation does nothing.
    */
    async fn incoming_message(&self, conn: ChannelConn<'_>, event: ChannelEvent) {}

    /**
    `disconnect` is called when the websocket client ceases to be
    connected, either gracefully or abruptly.
    */
    async fn disconnect(&self, conn: ChannelConn<'_>) {}
}
