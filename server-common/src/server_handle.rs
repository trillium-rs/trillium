use crate::CloneCounterObserver;
use async_cell::sync::AsyncCell;
use event_listener::{Event, EventListener};
use std::{
    fmt::{Debug, Formatter, Result},
    future::{Future, IntoFuture},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
};
use trillium::Info;
use trillium_http::Stopper;

/// A handle for a spawned trillium server. Returned by
/// [`Config::handle`][crate::Config::handle] and
/// [`Config::spawn`][crate::Config::spawn]
#[derive(Clone, Debug)]
pub struct ServerHandle {
    pub(crate) stopper: Stopper,
    pub(crate) info: Arc<AsyncCell<Info>>,
    pub(crate) completion: CompletionFuture,
    pub(crate) observer: CloneCounterObserver,
}

#[derive(Default)]
pub struct CompletionFuture(Arc<CompletionFutureInner>, Option<EventListener>);

impl Clone for CompletionFuture {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0), None)
    }
}

impl CompletionFuture {
    pub(crate) fn notify(self) {
        if !self.0.complete.swap(true, Ordering::SeqCst) {
            self.0.event.notify(usize::MAX);
        }
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.0.complete.load(Ordering::SeqCst)
    }

    pub(crate) fn new() -> Self {
        Self::default()
    }
}

pub struct CompletionFutureInner {
    complete: AtomicBool,
    event: Event,
}

impl Default for CompletionFutureInner {
    fn default() -> Self {
        Self {
            complete: AtomicBool::new(false),
            event: Event::new(),
        }
    }
}

impl Debug for CompletionFuture {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_tuple("CompletionFuture")
            .field(&self.0.complete)
            .finish()
    }
}

impl Future for CompletionFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Self(inner, option_listener) = &mut *self;
        loop {
            if inner.complete.load(Ordering::Relaxed) {
                return Poll::Ready(());
            }

            let listener = option_listener.get_or_insert_with(|| inner.event.listen());

            if inner.complete.load(Ordering::SeqCst) {
                return Poll::Ready(());
            }

            ready!(Pin::new(listener).poll(cx));

            *option_listener = None;
        }
    }
}

impl ServerHandle {
    /// await server start and retrieve the server's [`Info`]
    pub async fn info(&self) -> Info {
        self.info.get().await
    }

    /// stop server and wait for it to shut down gracefully
    pub async fn stop(&self) {
        self.stopper.stop();
        self.completion.clone().await
    }

    /// retrieves a clone of the [`Stopper`] used by this server
    pub fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }

    /// retrieves a [`CloneCounterObserver`] which can be used to
    /// monitor or modify the number of outstanding connections for
    /// the purposes of graceful shutdown.
    pub fn observer(&self) -> CloneCounterObserver {
        self.observer.clone()
    }

    /// checks whether this server has shut down. It's preferable to await
    /// this [`ServerHandle`] instead of polling this.
    pub fn is_running(&self) -> bool {
        !self.completion.is_complete()
    }
}

impl IntoFuture for ServerHandle {
    type Output = ();

    type IntoFuture = CompletionFuture;

    fn into_future(self) -> Self::IntoFuture {
        self.completion
    }
}
