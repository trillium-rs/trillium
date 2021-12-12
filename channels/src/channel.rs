use crate::{ChannelBroadcaster, ChannelCentral, ChannelEvent, ChannelHandler};
use std::{
    future::Future,
    ops::{Deref, DerefMut},
    pin::Pin,
};
use trillium::{async_trait, Conn, Handler, Info, Upgrade};
use trillium_websockets::{JsonHandler, WebSocket};

/**
Trillium handler containing a [`ChannelHandler`]

This is constructed from a [`ChannelHandler`] using [`Channel::new`]
and dereferences to that type.
*/
#[derive(Debug)]
pub struct Channel<CH>(WebSocket<JsonHandler<ChannelCentral<CH>>>);

#[async_trait]
impl<CH> Handler for Channel<CH>
where
    CH: ChannelHandler,
{
    async fn run(&self, conn: Conn) -> Conn {
        self.0.run(conn).await
    }

    fn init<'a>(&'a mut self, info: &'a mut Info) -> Pin<Box<dyn Future<Output = ()> + 'a>> {
        self.0.init(info)
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

impl<CH: ChannelHandler> Channel<CH> {
    /**
    Constructs a new trillium Channel handler from the provided
    [`ChannelHandler`] implementation
     */
    pub fn new(channel_handler: CH) -> Self {
        Self(WebSocket::new_json(ChannelCentral::new(channel_handler)))
    }

    /**
    Retrieve a ChannelBroadcaster that can be moved elsewhere or cloned
    in order to trigger channel events and listen for global events.
     */
    pub fn broadcaster(&self) -> ChannelBroadcaster {
        self.0.channel_broadcaster()
    }

    /**
    Send a ChannelEvent to all connected clients that subscribe to the topic
     */
    pub fn broadcast(&self, event: impl Into<ChannelEvent>) {
        self.0.broadcast(event);
    }
}

impl<CH> Deref for Channel<CH> {
    type Target = CH;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<CH> DerefMut for Channel<CH> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
