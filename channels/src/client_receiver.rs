use crate::{subscriptions::Subscriptions, ChannelEvent};
use async_broadcast::Receiver as BroadcastReceiver;
use async_channel::Receiver;
use futures_lite::{stream::Race, Stream, StreamExt};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

#[derive(Debug)]
pub struct ClientReceiver {
    subscriptions: Subscriptions,
    race: Race<BroadcastReceiver<ChannelEvent>, Receiver<ChannelEvent>>,
}

impl ClientReceiver {
    pub fn new(
        individual: Receiver<ChannelEvent>,
        broadcast: BroadcastReceiver<ChannelEvent>,
        subscriptions: Subscriptions,
    ) -> Self {
        Self {
            race: broadcast.race(individual),
            subscriptions,
        }
    }
}

impl Stream for ClientReceiver {
    type Item = ChannelEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match self.race.poll_next(cx) {
                Poll::Ready(Some(event)) if !self.subscriptions.subscribes(&event) => continue,
                other => break other,
            }
        }
    }
}
