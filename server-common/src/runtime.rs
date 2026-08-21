use futures_lite::Stream;
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, ready},
    time::Duration,
};

mod droppable_future;
pub use droppable_future::DroppableFuture;

mod runtime_trait;
pub use runtime_trait::RuntimeTrait;

mod fan_out;
pub use fan_out::FanOut;

mod object_safe_runtime;
use object_safe_runtime::ObjectSafeRuntime;

/// A type-erased [`RuntimeTrait`] implementation. Think of this as an `Arc<dyn RuntimeTrait>`
#[derive(Clone)]
pub struct Runtime(Arc<dyn ObjectSafeRuntime>);

impl Debug for Runtime {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Runtime").field(&format_args!("..")).finish()
    }
}

impl<R: RuntimeTrait> From<Arc<R>> for Runtime {
    fn from(value: Arc<R>) -> Self {
        Self(value)
    }
}

impl Runtime {
    /// Construct a new type-erased runtime object from any [`RuntimeTrait`] implementation.
    pub fn new(runtime: impl RuntimeTrait) -> Self {
        runtime.into() // we avoid re-arcing a Runtime by using Into::into
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

    /// Spawn a future on the runtime without a join handle.
    ///
    /// Cheaper than [`spawn`][Self::spawn] when the caller doesn't need the output or
    /// completion signal: no channel or join-handle allocation is made.
    pub fn spawn_detached<Fut>(&self, fut: Fut)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.0.spawn_detached(Box::pin(fut));
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
        let spawn_fut = self.0.spawn(Box::pin(SendOnComplete { fut, send }));
        DroppableFuture::new(Box::pin(async move {
            spawn_fut.await;
            receive.try_recv().ok()
        }))
    }

    fn spawn_detached<Fut>(&self, fut: Fut)
    where
        Fut: Future<Output = ()> + Send + 'static,
    {
        self.0.spawn_detached(Box::pin(fut));
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

    fn hook_signals(
        &self,
        signals: impl IntoIterator<Item = i32>,
    ) -> impl Stream<Item = i32> + Send + 'static {
        self.0.hook_signals(signals.into_iter().collect())
    }
}

pin_project_lite::pin_project! {
    /// Sends the inner future's output on completion. A hand-written combinator rather than an
    /// `async` block because a generator that captures `fut` and awaits it stores the future
    /// twice (capture slot + await slot), doubling the task allocation for every spawn.
    struct SendOnComplete<Fut: Future> {
        #[pin]
        fut: Fut,
        send: async_channel::Sender<Fut::Output>,
    }
}

impl<Fut: Future> Future for SendOnComplete<Fut> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.project();
        let output = ready!(this.fut.poll(cx));
        let _ = this.send.try_send(output);
        Poll::Ready(())
    }
}
