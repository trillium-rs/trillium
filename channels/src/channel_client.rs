use crate::{ChannelEvent, Version, client_receiver::ClientReceiver, subscriptions::Subscriptions};
use async_broadcast::{Receiver, Sender as BroadcastSender};
use async_channel::Sender;
use serde::Serialize;
use trillium::log_error;
use trillium_websockets::Message;

/// # Communicate with the connected client.
///
/// Note that although each client is unique and represents a specific
/// websocket connection, the ChannelClient can be cloned and moved
/// elsewhere if needed and any updates to the topic subscriptions
/// will be kept synchronized across clones.
#[derive(Debug, Clone)]
pub struct ChannelClient {
    subscriptions: Subscriptions,
    sender: Sender<ChannelEvent>,
    broadcast_sender: BroadcastSender<ChannelEvent>,
    version: Version,
}

impl ChannelClient {
    pub(crate) fn new(
        broadcast_sender: BroadcastSender<ChannelEvent>,
        broadcast_receiver: Receiver<ChannelEvent>,
        version: Version,
    ) -> (Self, ClientReceiver) {
        let (sender, individual) = async_channel::unbounded();
        let subscriptions = Subscriptions::default();
        (
            Self {
                subscriptions: subscriptions.clone(),
                sender,
                broadcast_sender,
                version,
            },
            ClientReceiver::new(individual, broadcast_receiver, subscriptions, version),
        )
    }

    /// Send a [`ChannelEvent`] to all connected clients. Note that
    /// these messages will only reach clients that subscribe to the
    /// event's topic.
    pub fn broadcast(&self, event: impl Into<ChannelEvent>) {
        let mut event = event.into();
        event.reference = None;
        log_error!(self.broadcast_sender.try_broadcast(event));
    }

    /// Send a [`ChannelEvent`] to this specific client. Note that
    /// this message will only be received if the client subscribes to
    /// the event's topic.
    pub async fn send_event(&self, event: impl Into<ChannelEvent>) {
        log_error!(self.sender.send(event.into()).await);
    }

    /// Send an ok reply in reference to the provided ChannelEvent
    /// with the provided response payload.
    ///
    /// Note that this sets the event as `"phx_reply"` and the payload as
    /// `{"status": "ok", "response": response }`, as well as setting the
    /// reference field.
    pub async fn reply_ok(&self, event: &ChannelEvent, payload: &impl Serialize) {
        #[derive(serde::Serialize)]
        struct Reply<'a, S> {
            status: &'static str,
            response: &'a S,
        }

        self.send_event(event.build_reply(
            "phx_reply",
            &Reply {
                status: "ok",
                response: payload,
            },
        ))
        .await
    }

    /// Send an error reply in reference to the provided ChannelEvent
    /// with the provided response payload.
    ///
    /// Note that this sets the event as `"phx_error"` as well as setting
    /// the reference field.
    pub async fn reply_error(&self, event: &ChannelEvent, error: &impl Serialize) {
        self.send_event(event.build_reply("phx_error", &error))
            .await
    }

    /// Join a topic, sending an ok reply with the provided optional
    /// value. This sends an ok reply to the client as well as adding the
    /// topic to the client's subscriptions.
    ///
    /// Use `&()` as the payload if no payload is needed.
    pub async fn allow_join(&self, event: &ChannelEvent, payload: &impl Serialize) {
        if event.event() != "phx_join" {
            log::error!(
                "allow_join called with an event other than phx_join: {:?}",
                event
            );
            return;
        }
        self.subscriptions.join(event.topic.to_string());
        self.reply_ok(event, payload).await;
    }

    /// Leave a topic as requested by the provided channel event,
    /// including the optional payload. This sends an ok reply to the
    /// client as well as removing the channel from the client's
    /// subscriptions.
    ///
    /// Use `&()` as the payload if no payload is needed.
    pub async fn allow_leave(&self, event: &ChannelEvent, payload: &impl Serialize) {
        if event.event() != "phx_leave" {
            log::error!(
                "allow_leave called with an event other than phx_leave: {:?}",
                event
            );
            return;
        }
        self.subscriptions.leave(&event.topic);
        self.reply_ok(event, payload).await;
    }

    /// Borrow this client's subscriptions
    pub fn subscriptions(&self) -> &Subscriptions {
        &self.subscriptions
    }

    pub(crate) fn deserialize(&self, message: Message) -> Option<ChannelEvent> {
        let string = message.to_text().ok()?;
        ChannelEvent::deserialize(string, self.version).ok()
    }
}
