use crate::{ChannelBroadcaster, ChannelCentral, ChannelEvent, ChannelHandler};
use std::ops::{Deref, DerefMut};
use trillium::{async_trait, Conn, Handler, Upgrade};
use trillium_websockets::{JsonHandler, WebSocket};

/**
Trillium handler for Channels, containing a ChannelHandler.

This is constructed from a [`ChannelHandler`] using [`Channels::new`]
and dereferences to that type.
*/
#[derive(Debug)]
pub struct Channels<CH>(WebSocket<JsonHandler<ChannelCentral<CH>>>);

#[async_trait]
impl<CH> Handler for Channels<CH>
where
    CH: ChannelHandler,
{
    async fn run(&self, conn: Conn) -> Conn {
        self.0.run(conn).await
    }

    async fn init(&mut self, info: &mut trillium::Info) {
        self.0.init(info).await;
    }

    async fn before_send(&self, conn: Conn) -> Conn {
        self.0.before_send(conn).await
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        self.0.has_upgrade(upgrade)
    }

    async fn upgrade(&self, upgrade: Upgrade) {
        self.0.upgrade(upgrade).await
    }
}

impl<CH: ChannelHandler> Channels<CH> {
    /**
    Constructs a new trillium Channels handler from the provided
    [`ChannelHandler`] implementation
     */
    pub fn new(channel_handler: CH) -> Self {
        Self(WebSocket::new_json(ChannelCentral::new(channel_handler)))
    }

    /**
    Retrieve a Broadcast sender that can be moved elsewhere or cloned
    in order to trigger channel events.
     */
    pub fn channel_broadcaster(&self) -> ChannelBroadcaster {
        self.0.channel_broadcaster()
    }

    /**
    Send a ChannelEvent to all connected clients that subscribe to the topic
     */
    pub fn broadcast(&self, event: impl Into<ChannelEvent>) {
        self.0.broadcast(event);
    }
}

impl<CH> Deref for Channels<CH> {
    type Target = CH;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<CH> DerefMut for Channels<CH> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
