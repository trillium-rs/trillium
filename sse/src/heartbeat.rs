use crate::Eventable;
use futures_lite::Stream;
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use trillium_server_common::Runtime;

/// An item from a stream that has been wrapped with a heartbeat.
///
/// The heartbeat variant carries an empty comment, which clients discard.
pub(crate) enum Heartbeat<E> {
    Event(E),
    Beat,
}

impl<E: Eventable> Eventable for Heartbeat<E> {
    fn data(&self) -> Option<&str> {
        match self {
            Self::Event(event) => event.data(),
            Self::Beat => None,
        }
    }

    fn comment(&self) -> Option<&str> {
        match self {
            Self::Event(event) => event.comment(),
            Self::Beat => Some(""),
        }
    }

    fn event_type(&self) -> Option<&str> {
        match self {
            Self::Event(event) => event.event_type(),
            Self::Beat => None,
        }
    }

    fn id(&self) -> Option<&str> {
        match self {
            Self::Event(event) => event.id(),
            Self::Beat => None,
        }
    }

    fn retry(&self) -> Option<Duration> {
        match self {
            Self::Event(event) => event.retry(),
            Self::Beat => None,
        }
    }
}

/// Yields a heartbeat whenever `interval` elapses without the wrapped stream producing an item.
pub(crate) struct WithHeartbeat<S> {
    stream: S,
    runtime: Runtime,
    interval: Duration,
    delay: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl<S> Debug for WithHeartbeat<S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("WithHeartbeat")
            .field("runtime", &self.runtime)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

impl<S> WithHeartbeat<S> {
    pub(crate) fn new(stream: S, runtime: Runtime, interval: Duration) -> Self {
        Self {
            stream,
            delay: delay(&runtime, interval),
            runtime,
            interval,
        }
    }

    fn reset_delay(&mut self) {
        self.delay = delay(&self.runtime, self.interval);
    }
}

fn delay(runtime: &Runtime, interval: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    let runtime = runtime.clone();
    Box::pin(async move { runtime.delay(interval).await })
}

impl<S: Stream + Unpin> Stream for WithHeartbeat<S> {
    type Item = Heartbeat<S::Item>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        match Pin::new(&mut this.stream).poll_next(cx) {
            Poll::Ready(Some(item)) => {
                this.reset_delay();
                Poll::Ready(Some(Heartbeat::Event(item)))
            }

            Poll::Ready(None) => Poll::Ready(None),

            Poll::Pending => match this.delay.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    this.reset_delay();
                    Poll::Ready(Some(Heartbeat::Beat))
                }
                Poll::Pending => Poll::Pending,
            },
        }
    }
}
