use async_io::Timer;
use async_task::Task;
use futures_lite::{FutureExt, Stream, StreamExt};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use trillium_server_common::{DroppableFuture, Runtime, RuntimeTrait};

/// Runtime for Smol
#[derive(Debug, Clone, Copy, Default)]
pub struct SmolRuntime(());

struct DetachOnDrop<Output>(Option<Task<Output>>);
impl<Output> Future for DetachOnDrop<Output> {
    type Output = Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(self.0.as_mut().unwrap()).poll(cx)
    }
}

impl<Output> Drop for DetachOnDrop<Output> {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.detach();
        }
    }
}

impl RuntimeTrait for SmolRuntime {
    fn spawn<Fut>(
        &self,
        fut: Fut,
    ) -> DroppableFuture<impl Future<Output = Option<Fut::Output>> + Send + 'static>
    where
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
    {
        let join_handle = DetachOnDrop(Some(async_global_executor::spawn(fut)));
        DroppableFuture::new(async move { join_handle.catch_unwind().await.ok() })
    }

    async fn delay(&self, duration: Duration) {
        Timer::after(duration).await;
    }

    fn interval(&self, period: Duration) -> impl Stream<Item = ()> + Send + 'static {
        Timer::interval(period).map(|_| ())
    }

    fn block_on<Fut: Future>(&self, fut: Fut) -> Fut::Output {
        async_global_executor::block_on(fut)
    }

    #[cfg(unix)]
    fn hook_signals(
        &self,
        signals: impl IntoIterator<Item = i32>,
    ) -> impl Stream<Item = i32> + Send + 'static {
        signal_hook_async_std::Signals::new(signals).unwrap()
    }
}

impl SmolRuntime {
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
        let join_handle = DetachOnDrop(Some(async_global_executor::spawn(fut)));
        DroppableFuture::new(async move { join_handle.catch_unwind().await.ok() })
    }

    /// Wake in this amount of wall time
    pub async fn delay(&self, duration: Duration) {
        Timer::after(duration).await;
    }

    /// Returns a [`Stream`] that yields a `()` on the provided period
    pub fn interval(&self, period: Duration) -> impl Stream<Item = ()> + Send + 'static {
        Timer::interval(period).map(|_| ())
    }

    /// Runtime implementation hook for blocking on a top level future.
    pub fn block_on<Fut: Future>(&self, fut: Fut) -> Fut::Output {
        RuntimeTrait::block_on(self, fut)
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

impl From<SmolRuntime> for Runtime {
    fn from(value: SmolRuntime) -> Self {
        Arc::new(value).into()
    }
}
