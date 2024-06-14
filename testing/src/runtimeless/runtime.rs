use futures_lite::{future, Stream};
use std::{future::Future, thread, time::Duration};
use trillium_server_common::{DroppableFuture, Runtime, RuntimeTrait};

/// a runtime that isn't a runtime
#[derive(Debug, Clone, Copy, Default)]
pub struct RuntimelessRuntime(());
impl RuntimeTrait for RuntimelessRuntime {
    fn spawn<Fut>(
        &self,
        fut: Fut,
    ) -> DroppableFuture<impl Future<Output = Option<Fut::Output>> + Send + 'static>
    where
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
    {
        let rt = *self;
        let (send, receive) = async_channel::bounded(1);
        thread::spawn(move || {
            let _ = send.send_blocking(rt.block_on(fut));
        });
        DroppableFuture::new(async move { receive.recv().await.ok() })
    }
    async fn delay(&self, duration: Duration) {
        let (send, receive) = async_channel::bounded(1);
        thread::spawn(move || {
            thread::sleep(duration);
            let _ = send.send_blocking(());
        });
        let _ = receive.recv().await;
    }

    fn interval(&self, period: Duration) -> impl Stream<Item = ()> + Send + 'static {
        let (send, receive) = async_channel::bounded(1);
        thread::spawn(move || loop {
            thread::sleep(period);
            if send.send_blocking(()).is_err() {
                break;
            }
        });

        receive
    }

    fn block_on<Fut: Future>(&self, fut: Fut) -> Fut::Output {
        future::block_on(fut)
    }
}
impl From<RuntimelessRuntime> for Runtime {
    fn from(value: RuntimelessRuntime) -> Self {
        Runtime::new(value)
    }
}
impl RuntimelessRuntime {
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
        let rt = *self;
        let (send, receive) = async_channel::bounded(1);
        thread::spawn(move || {
            let _ = send.send_blocking(rt.block_on(fut));
        });
        DroppableFuture::new(async move { receive.recv().await.ok() })
    }

    /// Wake in this amount of wall time
    pub async fn delay(&self, duration: Duration) {
        RuntimeTrait::delay(self, duration).await
    }

    /// Returns a [`Stream`] that yields a `()` on the provided period
    pub fn interval(&self, period: Duration) -> impl Stream<Item = ()> + Send + 'static {
        let (send, receive) = async_channel::bounded(1);
        thread::spawn(move || loop {
            thread::sleep(period);
            if send.is_closed() {
                break;
            }
            let _ = send.send_blocking(());
        });

        receive
    }

    /// Runtime implementation hook for blocking on a top level future.
    pub fn block_on<Fut: Future>(&self, fut: Fut) -> Fut::Output {
        future::block_on(fut)
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
