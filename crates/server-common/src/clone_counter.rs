use event_listener::{Event, EventListener};
use std::{
    fmt::{Debug, Formatter, Result},
    future::{Future, IntoFuture},
    pin::{pin, Pin},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{ready, Context, Poll},
};

pub struct CloneCounterInner {
    count: AtomicUsize,
    event: Event,
}

impl CloneCounterInner {
    fn new(start: usize) -> Self {
        Self {
            count: AtomicUsize::new(start),
            event: Event::new(),
        }
    }
}

impl Debug for CloneCounterInner {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.debug_struct("CloneCounterInner")
            .field("count", &self.count)
            .finish()
    }
}

/**
# an atomic counter that increments on clone & decrements on drop

One-indexed, because the first CloneCounter is included. If you don't
want the original to count, construct a [`CloneCounterObserver`]
instead and use [`CloneCounterObserver::counter`] to increment.

Awaiting a [`CloneCounter`] will be pending until it is the only remaining
counter and resolve to `()` when the count is 1.

*/

#[derive(Debug)]
pub struct CloneCounter(Arc<CloneCounterInner>);

impl Default for CloneCounter {
    fn default() -> Self {
        Self(Arc::new(CloneCounterInner::new(1)))
    }
}

impl CloneCounter {
    /// Constructs a new CloneCounter. Identical to CloneCounter::default()
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the current count. The first CloneCounter is one, so
    /// this can either be considered a one-indexed count of the
    /// total number of CloneCounters in memory
    pub fn current(&self) -> usize {
        self.0.current()
    }

    /// Manually decrement the count. This is useful when taking a
    /// clone of the counter that does not represent an increase in
    /// the underlying property or resource being counted. This is
    /// called automatically on drop and is usually unnecessary to
    /// call directly
    pub fn decrement(&self) {
        let previously = self.0.count.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(previously > 0);
        self.0.wake();
        if previously > 0 {
            log::trace!("decrementing from {} -> {}", previously, previously - 1);
        } else {
            log::trace!("decrementing from 0");
        }
    }

    /// Manually increment the count. unless paired with a decrement,
    /// this will prevent the clone counter from ever reaching
    /// zero. This is called automatically on clone.
    pub fn increment(&self) {
        let previously = self.0.count.fetch_add(1, Ordering::SeqCst);
        log::trace!("incrementing from {} -> {}", previously, previously + 1);
    }

    /// Returns an observer that can be cloned any number of times
    /// without modifying the clone counter. See
    /// [`CloneCounterObserver`] for more.
    pub fn observer(&self) -> CloneCounterObserver {
        CloneCounterObserver(Arc::clone(&self.0))
    }
}

impl IntoFuture for CloneCounter {
    type Output = ();

    type IntoFuture = CloneCounterFuture;

    fn into_future(self) -> Self::IntoFuture {
        CloneCounterFuture {
            inner: Arc::clone(&self.0),
            listener: EventListener::new(),
        }
    }
}

impl Future for &CloneCounter {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut listener = pin!(EventListener::new());
        loop {
            if 1 == self.0.current() {
                return Poll::Ready(());
            }

            if listener.is_listening() {
                ready!(listener.as_mut().poll(cx));
            } else {
                listener.as_mut().listen(&self.0.event)
            }
        }
    }
}
impl Clone for CloneCounter {
    fn clone(&self) -> Self {
        self.increment();
        Self(self.0.clone())
    }
}

impl Drop for CloneCounter {
    fn drop(&mut self) {
        self.decrement();
    }
}

impl CloneCounterInner {
    fn current(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn wake(&self) {
        self.event.notify(usize::MAX);
    }
}

impl PartialEq<usize> for CloneCounter {
    fn eq(&self, other: &usize) -> bool {
        self.current() == *other
    }
}

/**
An observer that can be cloned without modifying the clone
counter, but can be used to inspect its state and awaited

Zero-indexed, but each [`CloneCounter`] retrieved with
[`CloneCounterObserver::counter`] increments the count by 1.

Awaiting a [`CloneCounterObserver`] will be pending until all
associated [`CloneCounter`]s have been dropped, and will resolve to
`()` when the count is 0.

*/

#[derive(Debug)]
pub struct CloneCounterObserver(Arc<CloneCounterInner>);

impl Clone for CloneCounterObserver {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl Default for CloneCounterObserver {
    fn default() -> Self {
        Self(Arc::new(CloneCounterInner::new(0)))
    }
}

impl PartialEq<usize> for CloneCounterObserver {
    fn eq(&self, other: &usize) -> bool {
        self.current() == *other
    }
}

impl CloneCounterObserver {
    /// returns a new observer with a zero count. use [`CloneCounterObserver::counter`] to
    pub fn new() -> Self {
        Self::default()
    }
    /// returns the current counter value
    pub fn current(&self) -> usize {
        self.0.current()
    }

    /// creates a new CloneCounter from this observer, incrementing the count
    pub fn counter(&self) -> CloneCounter {
        let counter = CloneCounter(Arc::clone(&self.0));
        counter.increment();
        counter
    }
}

impl IntoFuture for CloneCounterObserver {
    type Output = ();

    type IntoFuture = CloneCounterFuture;

    fn into_future(self) -> Self::IntoFuture {
        CloneCounterFuture {
            listener: EventListener::new(),
            inner: self.0,
        }
    }
}

impl From<CloneCounter> for CloneCounterObserver {
    fn from(value: CloneCounter) -> Self {
        // value will be decremented on drop of the original
        Self(Arc::clone(&value.0))
    }
}

impl From<CloneCounterObserver> for CloneCounter {
    fn from(value: CloneCounterObserver) -> Self {
        let counter = Self(value.0);
        counter.increment();
        counter
    }
}

pin_project_lite::pin_project! {
    /// A future that waits for the clone counter to decrement to zero
    #[derive(Debug)]
    pub struct CloneCounterFuture {
        inner: Arc<CloneCounterInner>,
        #[pin]
        listener: EventListener,
    }
}

impl Clone for CloneCounterFuture {
    fn clone(&self) -> Self {
        let listener = EventListener::new();
        Self {
            inner: Arc::clone(&self.inner),
            listener,
        }
    }
}

impl Future for CloneCounterFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        loop {
            if 0 == this.inner.current() {
                return Poll::Ready(());
            };
            if this.listener.is_listening() {
                ready!(this.listener.as_mut().poll(cx));
            } else {
                this.listener.as_mut().listen(&this.inner.event);
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::clone_counter::CloneCounterObserver;

    use super::CloneCounter;
    use futures_lite::future::poll_once;
    use std::future::{Future, IntoFuture};
    use test_harness::test;

    fn block_on<F, Fut>(test: F)
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ()>,
    {
        trillium_testing::block_on(test());
    }

    #[test(harness = block_on)]
    async fn doctest_example() {
        let counter = CloneCounter::new();
        assert_eq!(counter.current(), 1);
        counter.await; // ready immediately

        let counter = CloneCounter::new();
        assert_eq!(counter.current(), 1);
        let clone = counter.clone();
        assert_eq!(counter.current(), 2);
        let clone2 = counter.clone();
        assert_eq!(counter.current(), 3);
        assert_eq!(poll_once(clone2.into_future()).await, None); // pending
        assert_eq!(counter.current(), 2);
        drop(clone);

        assert_eq!(counter.current(), 1);
        counter.await; // ready
    }

    #[test(harness = block_on)]
    async fn observer_into_and_from() {
        let counter = CloneCounter::new();
        assert_eq!(counter, 1);
        assert_eq!(counter.clone(), 2);
        assert_eq!(counter, 1);
        let observer = CloneCounterObserver::from(counter);
        assert_eq!(poll_once(observer.clone().into_future()).await, Some(()));
        assert_eq!(observer, 0);
        let counter = CloneCounter::from(observer);
        assert_eq!(counter, 1);
        assert_eq!(poll_once(counter.into_future()).await, Some(()));
    }

    #[test(harness = block_on)]
    async fn observer_test() {
        let counter = CloneCounter::new();
        assert_eq!(counter.current(), 1);
        counter.await; // ready immediately

        let counter = CloneCounter::new();
        let mut clones = Vec::new();
        let observer = counter.observer();
        assert_eq!(observer.current(), 1);
        for i in 1..=10 {
            clones.push(counter.clone());
            assert_eq!(counter.current(), 1 + i);
            assert_eq!(observer.current(), 1 + i);
        }

        let _observers = std::iter::repeat_with(|| observer.clone())
            .take(10)
            .collect::<Vec<_>>();
        assert_eq!(observer.current(), 11); // unchanged,

        let _observers = std::iter::repeat_with(|| counter.observer())
            .take(10)
            .collect::<Vec<_>>();
        assert_eq!(observer.current(), 11); // unchanged,

        for (i, clone) in clones.drain(..).enumerate() {
            assert_eq!(clone.current(), 11 - i);
            assert_eq!(observer.current(), 11 - i);
            assert_eq!(poll_once(&clone).await, None); // pending
            assert_eq!(poll_once(observer.clone().into_future()).await, None); // pending
            drop(clone);
            assert_eq!(counter.current(), 10 - i);
            assert_eq!(observer.current(), 10 - i);
        }

        assert_eq!(counter.current(), 1);
        assert_eq!(poll_once(counter.into_future()).await, Some(()));
        assert_eq!(observer.current(), 0);
        assert_eq!(poll_once(observer.into_future()).await, Some(()));
    }
}
