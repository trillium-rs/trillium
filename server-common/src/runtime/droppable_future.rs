use pin_project_lite::pin_project;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pin_project! {
    /// A wrapper type for futures that do not need to be polled but still can be awaited.
    ///
    /// This exists to silence the default `#[must_use]` that anonymous async functions return
    ///
    /// Futures contained by this type must conform to the semantics of trillium join handles described
    /// at [RuntimeTrait::spawn].
    #[derive(Debug, Clone)]
    pub struct DroppableFuture<T> {
        #[pin] future: T
    }
}
impl<T: Future> DroppableFuture<T> {
    /// Removes the #[must_use] for a future.
    ///
    /// This must only be called with a join-handle type future that does not depend on polling to
    /// execute.
    pub fn new(future: T) -> Self {
        Self { future }
    }

    /// Returns the inner future.
    pub fn into_inner(self) -> T {
        self.future
    }
}
impl<F: Future> Future for DroppableFuture<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.project().future.poll(cx)
    }
}
