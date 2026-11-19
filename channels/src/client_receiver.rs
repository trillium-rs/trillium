use crate::{ChannelEvent, Version, subscriptions::Subscriptions};
use async_broadcast::Receiver as BroadcastReceiver;
use async_channel::Receiver;
use futures_lite::{
    Stream, StreamExt,
    stream::{Map, Race},
};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use trillium_websockets::Message;

#[derive(Debug)]
pub(crate) enum OutboundEvent {
    Broadcast(ChannelEvent),
    Individual(ChannelEvent),
    Reply(ChannelEvent),
}

type BroadcastStream = Map<BroadcastReceiver<ChannelEvent>, fn(ChannelEvent) -> OutboundEvent>;

#[derive(Debug)]
pub struct ClientReceiver {
    subscriptions: Subscriptions,
    race: Pin<Box<Race<BroadcastStream, Receiver<OutboundEvent>>>>,
    version: Version,
}

impl ClientReceiver {
    pub(crate) fn new(
        individual: Receiver<OutboundEvent>,
        broadcast: BroadcastReceiver<ChannelEvent>,
        subscriptions: Subscriptions,
        version: Version,
    ) -> Self {
        Self {
            race: Box::pin(
                broadcast
                    .map(OutboundEvent::Broadcast as fn(_) -> _)
                    .race(individual),
            ),
            subscriptions,
            version,
        }
    }
}

impl Stream for ClientReceiver {
    type Item = Message;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let event = match self.race.poll_next(cx) {
                Poll::Ready(Some(OutboundEvent::Broadcast(mut event))) => {
                    if !self.subscriptions.subscribes(&event) {
                        continue;
                    }
                    // A Phoenix client drops a frame tied to another client's join.
                    event.join_reference = None;
                    event.reference = None;
                    event
                }
                Poll::Ready(Some(OutboundEvent::Individual(event))) => {
                    if !self.subscriptions.subscribes(&event) {
                        continue;
                    }
                    event
                }
                Poll::Ready(Some(OutboundEvent::Reply(event))) => event,
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
            };
            if let Ok(text) = event.serialize(self.version) {
                log::trace!(
                    "serialized {:?} with {:?} as {:?}",
                    event,
                    self.version,
                    text
                );
                return Poll::Ready(Some(Message::text(text)));
            }
        }
    }
}
