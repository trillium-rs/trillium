use futures_lite::future::FutureExt;
use std::{future::Future, time::Duration};
use trillium_server_common::{DroppableFuture, Runtime, RuntimeTrait, Stream};

/// async-std runtime
#[derive(Clone, Copy, Default, Debug)]
pub struct AsyncStdRuntime(());

impl RuntimeTrait for AsyncStdRuntime {
    fn spawn<Fut>(
        &self,
        fut: Fut,
    ) -> DroppableFuture<impl Future<Output = Option<Fut::Output>> + Send + 'static>
    where
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
    {
        let join_handle = async_std::task::spawn(fut);
        DroppableFuture::new(async move { join_handle.catch_unwind().await.ok() })
    }

    async fn delay(&self, duration: Duration) {
        async_std::task::sleep(duration).await
    }

    fn interval(&self, period: Duration) -> impl Stream<Item = ()> + Send + 'static {
        async_std::stream::interval(period)
    }

    fn block_on<Fut: Future>(&self, fut: Fut) -> Fut::Output {
        async_std::task::block_on(fut)
    }
}

impl AsyncStdRuntime {
    /// Spawn a future on the runtime, returning a future that has detach-on-drop semantics
    ///
    /// Spawned tasks conform to the following behavior:
    ///
    /// * detach on drop: If the returned [`DroppableFuture`] is dropped immediately, the task will
    ///   continue to execute until completion.
    ///
    /// * unwinding: If the spawned future panics, this must not propagate to the join handle.
    ///   Instead, the awaiting the join handle returns None in case of panic.
    pub fn spawn<Fut>(
        &self,
        fut: Fut,
    ) -> DroppableFuture<impl Future<Output = Option<Fut::Output>> + Send + 'static>
    where
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
    {
        let join_handle = async_std::task::spawn(fut);
        DroppableFuture::new(async move { join_handle.catch_unwind().await.ok() })
    }

    /// Wake in this amount of wall time
    pub async fn delay(&self, duration: Duration) {
        async_std::task::sleep(duration).await
    }

    /// Returns a [`Stream`] that yields a `()` on the provided period
    pub fn interval(&self, period: Duration) -> impl Stream<Item = ()> + Send + 'static {
        async_std::stream::interval(period)
    }

    /// Runtime implementation hook for blocking on a top level future.
    pub fn block_on<Fut: Future>(&self, fut: Fut) -> Fut::Output {
        async_std::task::block_on(fut)
    }

    /// Race a future against the provided duration, returning None in case of timeout.
    pub async fn timeout<Fut>(&self, duration: Duration, fut: Fut) -> Option<Fut::Output>
    where
        Fut: Future + Send,
        Fut::Output: Send + 'static,
    {
        RuntimeTrait::timeout(self, duration, fut).await
    }
}

impl From<AsyncStdRuntime> for Runtime {
    fn from(value: AsyncStdRuntime) -> Self {
        Runtime::new(value)
    }
}
