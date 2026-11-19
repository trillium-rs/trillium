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
enum OutboundEvent {
    Broadcast(ChannelEvent),
    Individual(ChannelEvent),
}

type BroadcastStream = Map<BroadcastReceiver<ChannelEvent>, fn(ChannelEvent) -> OutboundEvent>;
type IndividualStream = Map<Receiver<ChannelEvent>, fn(ChannelEvent) -> OutboundEvent>;

#[derive(Debug)]
pub struct ClientReceiver {
    subscriptions: Subscriptions,
    race: Pin<Box<Race<BroadcastStream, IndividualStream>>>,
    version: Version,
}

impl ClientReceiver {
    pub fn new(
        individual: Receiver<ChannelEvent>,
        broadcast: BroadcastReceiver<ChannelEvent>,
        subscriptions: Subscriptions,
        version: Version,
    ) -> Self {
        Self {
            race: Box::pin(
                broadcast
                    .map(OutboundEvent::Broadcast as fn(_) -> _)
                    .race(individual.map(OutboundEvent::Individual as fn(_) -> _)),
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
                    // A Phoenix client drops a frame tied to another client's join.
                    event.join_reference = None;
                    event.reference = None;
                    event
                }
                Poll::Ready(Some(OutboundEvent::Individual(event))) => event,
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(None),
            };
            if self.subscriptions.subscribes(&event)
                && let Ok(text) = event.serialize(self.version)
            {
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
