use crate::{ChannelClient, ChannelEvent};
use serde::Serialize;
use std::ops::{Deref, DerefMut};
use trillium_websockets::WebSocketConn;

/// A ChannelConn is a wrapper around a [`WebSocketConn`] that also
/// contains a [`ChannelClient`]
///
/// It that provides convenient access to functions from the ChannelClient
/// held in the WebSocketConn's TypeSet, and dereferences to the
/// `WebSocketConn` for other functionality.
#[derive(Debug)]
pub struct ChannelConn<'a> {
    pub(crate) conn: &'a mut WebSocketConn,
}

macro_rules! channel_client {
    ($conn:expr_2021) => {
        match $conn.client() {
            Some(client) => client,
            None => {
                log::error!("could not unwrap client on {}:{}", file!(), line!());
                return;
            }
        }
    };
}

impl ChannelConn<'_> {
    /// Borrow the channel client
    pub fn client(&self) -> Option<&ChannelClient> {
        self.state()
    }

    /// Borrow the websocket conn
    pub fn conn(&self) -> &WebSocketConn {
        self
    }

    /// Send a [`ChannelEvent`] to all connected clients. Note that
    /// these messages will only reach clients that subscribe to the
    /// event's topic.
    pub fn broadcast(&self, event: impl Into<ChannelEvent>) {
        channel_client!(self).broadcast(event);
    }

    /// Send a [`ChannelEvent`] to this specific client. Note that
    /// this message will only be received if the client subscribes to
    /// the event's topic.
    pub async fn send_event(&self, event: impl Into<ChannelEvent>) {
        channel_client!(self).send_event(event).await;
    }

    /// Send a `phx_reply` with status `"ok"` and the provided response payload.
    /// The reply preserves the request's references and does not require a topic subscription.
    pub async fn reply_ok(&self, event: &ChannelEvent, response: &impl Serialize) {
        channel_client!(self).reply_ok(event, response).await;
    }

    /// Reject a join or push with a `phx_reply` with status `"error"`.
    ///
    /// The reply preserves the request's references and reaches the client even
    /// if it has not joined the topic. This does not change its subscriptions.
    pub async fn reject(&self, event: &ChannelEvent, error: &impl Serialize) {
        channel_client!(self).reject(event, error).await;
    }

    /// Mark the client's channel for this event's topic as errored, by sending a `phx_error`.
    ///
    /// The Phoenix JavaScript client treats this as a crashed channel and rejoins it. The event
    /// is only delivered if the client subscribes to the topic. To reject an individual join or
    /// push, use [`Self::reject`] instead.
    pub async fn mark_channel_errored(&self, event: &ChannelEvent, error: &impl Serialize) {
        channel_client!(self)
            .mark_channel_errored(event, error)
            .await;
    }

    /// Mark the client's channel for this event's topic as errored.
    #[deprecated(
        since = "0.5.0",
        note = "to refuse a join or push, use `reject`; to put the client's channel in an error \
                state, use `mark_channel_errored`"
    )]
    pub async fn reply_error(&self, event: &ChannelEvent, error: &impl Serialize) {
        channel_client!(self)
            .mark_channel_errored(event, error)
            .await;
    }

    /// Join a topic, sending an ok reply with the provided optional
    /// value. This sends an ok reply to the client as well as adding the
    /// topic to the client's subscriptions.
    pub async fn allow_join(&self, event: &ChannelEvent, value: &impl Serialize) {
        channel_client!(self).allow_join(event, value).await;
    }

    /// Leave a topic as requested by the provided channel event,
    /// including the optional payload. This sends an ok reply to the
    /// client as well as removing the channel from the client's
    /// subscriptions.
    pub async fn allow_leave(&self, event: &ChannelEvent, payload: &impl Serialize) {
        channel_client!(self).allow_leave(event, payload).await;
    }
}

impl Deref for ChannelConn<'_> {
    type Target = WebSocketConn;

    fn deref(&self) -> &Self::Target {
        &*self.conn
    }
}

impl DerefMut for ChannelConn<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.conn
    }
}
