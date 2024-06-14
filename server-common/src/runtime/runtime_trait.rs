use super::{DroppableFuture, Runtime};
use futures_lite::{FutureExt, Stream};
use std::{future::Future, time::Duration};

/// A trait that covers async runtime behavior.
///
/// You likely do not need to name this type. For a type-erased runtime, see [`Runtime`]
pub trait RuntimeTrait: Into<Runtime> + Clone + Send + Sync + 'static {
    /// Spawn a future on the runtime, returning a future that has detach-on-drop semantics
    ///
    /// As the various runtimes each has different behavior for spawn, implementations of this trait
    /// are expected to conform to the following:
    ///
    /// * detach on drop: If the returned [`DroppableFuture`] is dropped immediately, the task will
    ///   continue to execute until completion.
    ///
    /// * unwinding: If the spawned future panics, this must not propagate to the join handle.
    ///   Instead, the awaiting the join handle returns None in case of panic.
    fn spawn<Fut>(
        &self,
        fut: Fut,
    ) -> DroppableFuture<impl Future<Output = Option<Fut::Output>> + Send + 'static>
    where
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static;

    /// Wake in this amount of wall time
    fn delay(&self, duration: Duration) -> impl Future<Output = ()> + Send;

    /// Returns a [`Stream`] that yields a `()` on the provided period
    fn interval(&self, period: Duration) -> impl Stream<Item = ()> + Send + 'static;

    /// Runtime implementation hook for blocking on a top level future.
    fn block_on<Fut>(&self, fut: Fut) -> Fut::Output
    where
        Fut: Future;

    /// Race a future against the provided duration, returning None in case of timeout.
    fn timeout<'runtime, 'fut, Fut>(
        &'runtime self,
        duration: Duration,
        fut: Fut,
    ) -> impl Future<Output = Option<Fut::Output>> + Send + 'fut
    where
        Fut: Future + Send + 'fut,
        Fut::Output: Send + 'static,
        'runtime: 'fut,
    {
        async move { Some(fut.await) }.race(async move {
            self.delay(duration).await;
            None
        })
    }
}
