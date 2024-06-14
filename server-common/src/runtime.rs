use futures_lite::Stream;
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

mod droppable_future;
pub use droppable_future::DroppableFuture;

mod runtime_trait;
pub use runtime_trait::RuntimeTrait;

mod object_safe_runtime;
use object_safe_runtime::ObjectSafeRuntime;

/// A type-erased [`RuntimeTrait`] implementation. Think of this as an `Arc<dyn RuntimeTrait>`
#[derive(Clone)]
pub struct Runtime(Arc<dyn ObjectSafeRuntime>);

impl Debug for Runtime {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Runtime").field(&"..").finish()
    }
}

impl Runtime {
    /// Construct a new type-erased runtime object from any [`RuntimeTrait`] implementation.
    ///
    /// Prefer using [`from`][From::from]/[`into`][Into::into] if you don't have a concrete
    /// `RuntimeTrait` in order to avoid double-arc-ing a Runtime.
    pub fn new(runtime: impl RuntimeTrait) -> Self {
        Self(Arc::new(runtime))
    }

    /// Spawn a future on the runtime, returning a future that has detach-on-drop semantics
    ///
    /// Spawned tasks conform to the following behavior:
    ///
    /// * detach on drop: If the returned [`DroppableFuture`] is dropped immediately, the task will
    ///   continue to execute until completion.
    ///
    /// * unwinding: If the spawned future panics, this must not propagate to the join handle.
    ///   Instead, the awaiting the join handle returns None in case of panic.
    pub fn spawn<Output: Send + 'static>(
        &self,
        fut: impl Future<Output = Output> + Send + 'static,
    ) -> DroppableFuture<Pin<Box<dyn Future<Output = Option<Output>> + Send + 'static>>> {
        let fut = RuntimeTrait::spawn(self, fut).into_inner();
        DroppableFuture::new(Box::pin(fut))
    }

    /// Wake in this amount of wall time
    pub async fn delay(&self, duration: Duration) {
        RuntimeTrait::delay(self, duration).await
    }

    /// Returns a [`Stream`] that yields a `()` on the provided period
    pub fn interval(&self, period: Duration) -> impl Stream<Item = ()> + Send + '_ {
        RuntimeTrait::interval(self, period)
    }

    /// Runtime implementation hook for blocking on a top level future.
    pub fn block_on<Fut>(&self, fut: Fut) -> Fut::Output
    where
        Fut: Future,
    {
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

impl RuntimeTrait for Runtime {
    async fn delay(&self, duration: Duration) {
        self.0.delay(duration).await
    }

    fn interval(&self, period: Duration) -> impl Stream<Item = ()> + Send + 'static {
        self.0.interval(period)
    }

    fn spawn<Fut>(
        &self,
        fut: Fut,
    ) -> DroppableFuture<impl Future<Output = Option<Fut::Output>> + Send + 'static>
    where
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
    {
        let (send, receive) = async_channel::bounded(1);
        let spawn_fut = self.0.spawn(Box::pin(async move {
            let _ = send.try_send(fut.await);
        }));
        DroppableFuture::new(Box::pin(async move {
            spawn_fut.await;
            receive.try_recv().ok()
        }))
    }

    fn block_on<Fut>(&self, fut: Fut) -> Fut::Output
    where
        Fut: Future,
    {
        let (send, receive) = std::sync::mpsc::channel();
        self.0.block_on(Box::pin(async move {
            let _ = send.send(fut.await);
        }));
        receive.recv().unwrap()
    }
}
