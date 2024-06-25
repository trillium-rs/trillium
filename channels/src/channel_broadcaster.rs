use crate::ChannelEvent;
use async_broadcast::{InactiveReceiver, Receiver as ActiveReceiver, Sender};
use futures_lite::Stream;
use std::{
    mem,
    pin::Pin,
    task::{Context, Poll},
};

/// Channel-wide event broadcaster and subscriber
///
/// This can be cloned and stored elsewhere in an application in order to
/// send events to connected channel clients. Retrieve a [`ChannelBroadcaster`] from a
/// [`Channel`](crate::Channel) by calling
/// [`Channel::broadcaster`](crate::Channel::broadcaster)
///
/// ChannelBroadcaster also implements [`Stream`] so that your application
/// can listen in on ChannelEvents happening elsewhere. This might be used
/// for spawning a task to log events, or synchronizing events between
/// servers.
#[derive(Clone, Debug)]
pub struct ChannelBroadcaster {
    sender: Sender<ChannelEvent>,
    receiver: Receiver<ChannelEvent>,
}

#[derive(Debug)]
enum Receiver<C> {
    Active(ActiveReceiver<C>),
    Inactive(InactiveReceiver<C>),
    Activating,
}

impl<C> Clone for Receiver<C> {
    fn clone(&self) -> Self {
        match self {
            Self::Active(active) => Self::Inactive(active.clone().deactivate()),
            Self::Inactive(inactive) => Self::Inactive(inactive.clone()),
            Self::Activating => Self::Activating, // should not be reachable
        }
    }
}

impl<C> Stream for Receiver<C>
where
    C: Clone + std::fmt::Debug,
{
    type Item = C;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.activate();

        match &mut *self {
            Receiver::Active(a) => Pin::new(a).poll_next(cx),
            _ => Poll::Ready(None), // unreachable, but why panic when we can just end the stream?
        }
    }
}

impl<C> Receiver<C>
where
    C: Clone,
{
    fn activate(&mut self) {
        if let Receiver::Inactive(_) = self {
            if let Receiver::Inactive(inactive) = mem::replace(self, Self::Activating) {
                *self = Receiver::Active(inactive.activate());
            };
        }
    }
}

impl ChannelBroadcaster {
    pub(crate) fn new(
        sender: Sender<ChannelEvent>,
        receiver: InactiveReceiver<ChannelEvent>,
    ) -> Self {
        Self {
            sender,
            receiver: Receiver::Inactive(receiver),
        }
    }

    /// Send this ChannelEvent to all subscribed channel clients
    pub fn broadcast(&self, event: impl Into<ChannelEvent>) {
        // we don't care about whether there are any connected clients
        // here, so we ignore error results.
        self.sender.try_broadcast(event.into()).ok();
    }

    /// Returns the number of connected clients. Note that the number of
    /// clients listening on any given channel will likely be smaller than
    /// this, and currently that number is not available.
    pub fn connected_clients(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Stream for ChannelBroadcaster {
    type Item = ChannelEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.receiver).poll_next(cx)
    }
}
