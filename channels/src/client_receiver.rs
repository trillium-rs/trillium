use crate::{subscriptions::Subscriptions, ChannelEvent, Version};
use async_broadcast::Receiver as BroadcastReceiver;
use async_channel::Receiver;
use futures_lite::{stream::Race, Stream, StreamExt};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use trillium_websockets::Message;

#[derive(Debug)]
pub struct ClientReceiver {
    subscriptions: Subscriptions,
    race: Race<BroadcastReceiver<ChannelEvent>, Receiver<ChannelEvent>>,
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
            race: broadcast.race(individual),
            subscriptions,
            version,
        }
    }
}

impl Stream for ClientReceiver {
    type Item = Message;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match self.race.poll_next(cx) {
                Poll::Ready(Some(event)) if !self.subscriptions.subscribes(&event) => continue,
                Poll::Ready(Some(event)) => {
                    if let Ok(text) = event.serialize(self.version) {
                        log::trace!(
                            "serialized {:?} with {:?} as {:?}",
                            event,
                            &self.version,
                            &text
                        );
                        break Poll::Ready(Some(Message::Text(text)));
                    }
                }
                Poll::Pending => break Poll::Pending,
                Poll::Ready(None) => break Poll::Ready(None),
            }
        }
    }
}
