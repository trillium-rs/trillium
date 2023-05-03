use crate::CloneCounterObserver;
use async_cell::sync::AsyncCell;
use std::{
    fmt::{Debug, Formatter, Result},
    future::{Future, IntoFuture},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Context, Poll},
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

#[derive(Clone, Default)]
pub struct CompletionFuture(Arc<CompletionFutureInner>);

impl CompletionFuture {
    pub(crate) fn notify(self) {
        if !self.0.complete.swap(true, Ordering::SeqCst) {
            self.0.waker_set.notify_all();
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
    waker_set: waker_set::WakerSet,
}

impl Default for CompletionFutureInner {
    fn default() -> Self {
        Self {
            complete: AtomicBool::new(false),
            waker_set: waker_set::WakerSet::new(),
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

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.0.complete.load(Ordering::SeqCst) {
            Poll::Ready(())
        } else {
            let key = self.0.waker_set.insert(cx);
            if self.0.complete.load(Ordering::SeqCst) {
                self.0.waker_set.cancel(key);
                Poll::Ready(())
            } else {
                Poll::Pending
            }
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

    /// checks whether this server has shut down. It's preferable to
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
