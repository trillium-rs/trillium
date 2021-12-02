use crate::{
    client_receiver::ClientReceiver, ChannelBroadcaster, ChannelClient, ChannelConn, ChannelEvent,
    ChannelHandler,
};
use async_broadcast::{InactiveReceiver, Sender};
use std::ops::{Deref, DerefMut};
use trillium::async_trait;
use trillium_websockets::{tungstenite::protocol::CloseFrame, JsonWebSocketHandler, WebSocketConn};

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

    fn build_client(&self) -> (ChannelClient, ClientReceiver) {
        ChannelClient::new(
            self.broadcast_sender.clone(),
            self.broadcast_receiver.activate_cloned(),
        )
    }
}

#[async_trait]
impl<CH> JsonWebSocketHandler for ChannelCentral<CH>
where
    CH: ChannelHandler,
{
    type OutboundMessage = ChannelEvent;
    type InboundMessage = ChannelEvent;
    type StreamType = ClientReceiver;

    async fn connect(&self, conn: &mut WebSocketConn) -> Self::StreamType {
        let (client, receiver) = self.build_client();
        conn.set_state(client);

        // this is always ok because we just set the client in state
        self.handler.connect(ChannelConn { conn }).await;
        receiver
    }

    async fn receive_message(&self, event: Self::InboundMessage, conn: &mut WebSocketConn) {
        match (&*event.topic, &*event.event) {
            ("phoenix", "heartbeat") => {
                log::trace!("heartbeat");
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
