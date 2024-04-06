use crate::{
    client_receiver::ClientReceiver, ChannelBroadcaster, ChannelClient, ChannelConn, ChannelEvent,
    ChannelHandler, Version,
};
use async_broadcast::{InactiveReceiver, Sender};
use querystrong::QueryStrong;
use std::ops::{Deref, DerefMut};
use trillium_websockets::{tungstenite::protocol::CloseFrame, WebSocketConn, WebSocketHandler};

const CHANNEL_CAP: usize = 10;

#[derive(Debug)]
pub(crate) struct ChannelCentral<CH> {
    handler: CH,
    broadcast_sender: Sender<ChannelEvent>,
    broadcast_receiver: InactiveReceiver<ChannelEvent>,
}

impl<CH> ChannelCentral<CH>
where
    CH: ChannelHandler,
{
    pub(crate) fn new(handler: CH) -> Self {
        let (mut broadcast_sender, broadcast_receiver) = async_broadcast::broadcast(CHANNEL_CAP);
        broadcast_sender.set_overflow(true);
        let broadcast_receiver = broadcast_receiver.deactivate();
        Self {
            handler,
            broadcast_sender,
            broadcast_receiver,
        }
    }

    pub(crate) fn channel_broadcaster(&self) -> ChannelBroadcaster {
        ChannelBroadcaster::new(
            self.broadcast_sender.clone(),
            self.broadcast_receiver.clone(),
        )
    }

    pub(crate) fn broadcast(&self, event: impl Into<ChannelEvent>) {
        trillium::log_error!(self.broadcast_sender.try_broadcast(event.into()));
    }

    fn build_client(&self, version: Version) -> (ChannelClient, ClientReceiver) {
        ChannelClient::new(
            self.broadcast_sender.clone(),
            self.broadcast_receiver.activate_cloned(),
            version,
        )
    }
}

macro_rules! unwrap_or_return {
    ($option:expr) => {
        unwrap_or_return!($option, ())
    };

    ($option:expr, $value:expr) => {
        match $option {
            Some(value) => value,
            None => return $value,
        }
    };
}

impl<CH> WebSocketHandler for ChannelCentral<CH>
where
    CH: ChannelHandler,
{
    type OutboundStream = ClientReceiver;

    async fn connect(
        &self,
        mut conn: WebSocketConn,
    ) -> Option<(WebSocketConn, Self::OutboundStream)> {
        let vsn = match QueryStrong::parse(conn.querystring())
            .unwrap_or_default()
            .get_str("vsn")
        {
            Some(version) => version.into(),
            _ => Version::V1,
        };

        let (client, receiver) = self.build_client(vsn);

        conn.set_state(client);

        // this is always ok because we just set the client in state
        self.handler.connect(ChannelConn { conn: &mut conn }).await;
        Some((conn, receiver))
    }

    async fn inbound(&self, message: trillium_websockets::Message, conn: &mut WebSocketConn) {
        let client = unwrap_or_return!(conn.state::<ChannelClient>());

        log::trace!("received message as {:?}", &message);
        let event = unwrap_or_return!(client.deserialize(message));

        log::trace!("deserialized message as {:?}", &event);
        match (&*event.topic, &*event.event) {
            ("phoenix", "heartbeat") => {
                log::trace!("heartbeat");
                client.reply_ok(&event, &()).await;
            }

            (_, "phx_join") => {
                self.handler.join_channel(ChannelConn { conn }, event).await;
            }

            (_, "phx_leave") => {
                self.handler
                    .leave_channel(ChannelConn { conn }, event)
                    .await;
            }

            _ => {
                self.handler
                    .incoming_message(ChannelConn { conn }, event)
                    .await;
            }
        }
    }

    async fn disconnect(
        &self,
        conn: &mut WebSocketConn,
        _close_frame: Option<CloseFrame<'static>>,
    ) {
        self.handler.disconnect(ChannelConn { conn }).await
    }
}

impl<CH> Deref for ChannelCentral<CH> {
    type Target = CH;

    fn deref(&self) -> &Self::Target {
        &self.handler
    }
}

impl<CH> DerefMut for ChannelCentral<CH> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.handler
    }
}
