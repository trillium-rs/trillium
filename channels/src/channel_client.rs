use crate::{
    ChannelEvent, Version,
    client_receiver::{ClientReceiver, OutboundEvent},
    subscriptions::Subscriptions,
};
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
    sender: Sender<OutboundEvent>,
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
        log_error!(
            self.sender
                .send(OutboundEvent::Individual(event.into()))
                .await
        );
    }

    /// Send a `phx_reply` with status `"ok"` and the provided response payload.
    /// The reply preserves the request's references and does not require a topic subscription.
    pub async fn reply_ok(&self, event: &ChannelEvent, payload: &impl Serialize) {
        self.reply(event, "ok", payload).await;
    }

    /// Reject a join or push with a `phx_reply` with status `"error"`.
    ///
    /// The reply preserves the request's references and reaches the client even
    /// if it has not joined the topic. This does not change its subscriptions.
    pub async fn reject(&self, event: &ChannelEvent, error: &impl Serialize) {
        self.reply(event, "error", error).await;
    }

    async fn reply(&self, event: &ChannelEvent, status: &'static str, response: &impl Serialize) {
        #[derive(serde::Serialize)]
        struct Reply<'a, S> {
            status: &'static str,
            response: &'a S,
        }

        match event.try_build_reply("phx_reply", &Reply { status, response }) {
            Ok(reply) => log_error!(self.sender.send(OutboundEvent::Reply(reply)).await),
            Err(error) => log::error!("could not serialize channel reply: {error}"),
        }
    }

    /// Mark the client's channel for this event's topic as errored, by sending a `phx_error`.
    ///
    /// The Phoenix JavaScript client treats this as a crashed channel and rejoins it. The event
    /// is only delivered if the client subscribes to the topic. To reject an individual join or
    /// push, use [`Self::reject`] instead.
    pub async fn mark_channel_errored(&self, event: &ChannelEvent, error: &impl Serialize) {
        match event.try_build_reply("phx_error", error) {
            Ok(reply) => self.send_event(reply).await,
            Err(error) => log::error!("could not serialize channel error: {error}"),
        }
    }

    /// Mark the client's channel for this event's topic as errored.
    #[deprecated(
        since = "0.5.0",
        note = "to refuse a join or push, use `reject`; to put the client's channel in an error \
                state, use `mark_channel_errored`"
    )]
    pub async fn reply_error(&self, event: &ChannelEvent, error: &impl Serialize) {
        self.mark_channel_errored(event, error).await;
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
