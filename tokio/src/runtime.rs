use std::{future::Future, sync::Arc, time::Duration};
use tokio::{runtime::Handle, time};
use tokio_stream::{Stream, StreamExt, wrappers::IntervalStream};
use trillium_server_common::{DroppableFuture, Runtime, RuntimeTrait};

#[derive(Debug, Clone)]
enum Inner {
    AlreadyRunning(Handle),
    Owned(Arc<tokio::runtime::Runtime>),
}

/// tokio runtime
#[derive(Clone, Debug)]
pub struct TokioRuntime(Inner);

impl Default for TokioRuntime {
    fn default() -> Self {
        match Handle::try_current() {
            Ok(handle) => Self(Inner::AlreadyRunning(handle)),
            _ => Self(Inner::Owned(Arc::new(
                tokio::runtime::Runtime::new().unwrap(),
            ))),
        }
    }
}

impl RuntimeTrait for TokioRuntime {
    fn spawn<Fut>(
        &self,
        fut: Fut,
    ) -> DroppableFuture<impl Future<Output = Option<Fut::Output>> + Send + 'static>
    where
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
    {
        let join_handle = match &self.0 {
            Inner::AlreadyRunning(handle) => handle.spawn(fut),
            Inner::Owned(runtime) => runtime.spawn(fut),
        };
        DroppableFuture::new(async move { join_handle.await.ok() })
    }

    async fn delay(&self, duration: Duration) {
        time::sleep(duration).await;
    }

    fn interval(&self, period: Duration) -> impl Stream<Item = ()> + Send + 'static {
        IntervalStream::new(time::interval(period)).map(|_| ())
    }

    fn block_on<Fut: Future>(&self, fut: Fut) -> Fut::Output {
        match &self.0 {
            Inner::AlreadyRunning(handle) => handle.block_on(fut),
            Inner::Owned(runtime) => runtime.block_on(fut),
        }
    }
}

impl TokioRuntime {
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
    ) -> DroppableFuture<impl Future<Output = Option<Fut::Output>> + Send + 'static + use<Fut>>
    where
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
    {
        let join_handle = match &self.0 {
            Inner::AlreadyRunning(handle) => handle.spawn(fut),
            Inner::Owned(runtime) => runtime.spawn(fut),
        };
        DroppableFuture::new(async move { join_handle.await.ok() })
    }

    /// wake in this amount of wall time
    pub async fn delay(&self, duration: Duration) {
        time::sleep(duration).await;
    }

    /// Returns a [`Stream`] that yields a `()` on the provided period
    pub fn interval(&self, period: Duration) -> impl Stream<Item = ()> + Send + 'static + use<> {
        IntervalStream::new(time::interval(period)).map(|_| ())
    }

    /// Runtime implementation hook for blocking on a top level future.
    pub fn block_on<Fut: Future>(&self, fut: Fut) -> Fut::Output {
        match &self.0 {
            Inner::AlreadyRunning(handle) => handle.block_on(fut),
            Inner::Owned(runtime) => runtime.block_on(fut),
        }
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

impl From<TokioRuntime> for Runtime {
    fn from(value: TokioRuntime) -> Self {
        Runtime::new(value)
    }
}
