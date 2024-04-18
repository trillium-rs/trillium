use super::RuntimeTrait;
use futures_lite::Stream;
use std::{future::Future, pin::Pin, time::Duration};

pub(super) trait ObjectSafeRuntime: Send + Sync + 'static {
    fn spawn(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
    ) -> Pin<Box<dyn Future<Output = Option<()>> + Send + 'static>>;
    fn delay<'runtime, 'fut>(
        &'runtime self,
        duration: Duration,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'fut>>
    where
        'runtime: 'fut,
        Self: 'fut;
    fn interval(&self, period: Duration) -> Pin<Box<dyn Stream<Item = ()> + Send + 'static>>;
    fn block_on<'runtime, 'fut>(&'runtime self, fut: Pin<Box<dyn Future<Output = ()> + 'fut>>)
    where
        'runtime: 'fut,
        Self: 'fut;
}

impl<R> ObjectSafeRuntime for R
where
    R: RuntimeTrait + Send + Sync + 'static,
{
    fn spawn(
        &self,
        fut: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
    ) -> Pin<Box<dyn Future<Output = Option<()>> + Send + 'static>> {
        Box::pin(RuntimeTrait::spawn(self, Box::pin(fut)))
    }

    fn delay<'runtime, 'fut>(
        &'runtime self,
        duration: Duration,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'fut>>
    where
        'runtime: 'fut,
        Self: 'fut,
    {
        Box::pin(RuntimeTrait::delay(self, duration))
    }

    fn interval(&self, period: Duration) -> Pin<Box<dyn Stream<Item = ()> + Send + 'static>> {
        Box::pin(RuntimeTrait::interval(self, period))
    }

    fn block_on<'runtime, 'fut>(&'runtime self, fut: Pin<Box<dyn Future<Output = ()> + 'fut>>)
    where
        'runtime: 'fut,
        Self: 'fut,
    {
        RuntimeTrait::block_on(self, fut)
    }
}
