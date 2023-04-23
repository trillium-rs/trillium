use futures_lite::Future;
use std::{
    fmt::{Debug, Formatter, Result},
    future::IntoFuture,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use waker_set::WakerSet;

pub struct CloneCounterInner {
    count: AtomicUsize,
    waker: WakerSet,
}

impl Default for CloneCounterInner {
    fn default() -> Self {
        Self {
            count: Default::default(),
            waker: WakerSet::new(),
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

```rust
# trillium_testing::block_on(async {
# use trillium_server_common::CloneCounter;
use futures_lite::future::poll_once;
let counter = CloneCounter::new();
assert_eq!(counter.current(), 0);
counter.await; // ready immediately

let counter = CloneCounter::new();
assert_eq!(counter.current(), 0);
let clone = counter.clone();
assert_eq!(counter.current(), 1);
let clone2 = counter.clone();
assert_eq!(counter.current(), 2);
assert_eq!(poll_once(clone2).await, None); // pending
assert_eq!(counter.current(), 1);
std::mem::drop(clone);

assert_eq!(counter.current(), 0);
counter.await; // ready

# });
```
*/

#[derive(Default, Debug)]
pub struct CloneCounter(Arc<CloneCounterInner>);

impl CloneCounter {
    /// Constructs a new CloneCounter. Identical to CloneCounter::default()
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the current count. The first CloneCounter is zero, so
    /// this can either be considered a zero-indexed count of the
    /// total number of CloneCounters in memory, or a one-indexed
    /// count of the number of non-original clones.
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
    /// without modifying the clone counter
    pub fn observer(&self) -> CloneCounterObserver {
        CloneCounterObserver(Arc::clone(&self.0))
    }
}

impl Future for CloneCounter {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if 0 == self.0.current() {
            Poll::Ready(())
        } else {
            self.0.register(cx);
            if 0 == self.0.current() {
                Poll::Ready(())
            } else {
                Poll::Pending
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

    fn register(&self, cx: &Context<'_>) {
        self.waker.insert(cx);
    }

    fn wake(&self) {
        self.waker.notify_all();
    }
}

/// An observer that can be cloned without modifying the clone
/// counter, but can be used to inspect its state and awaited
#[derive(Clone, Debug)]
pub struct CloneCounterObserver(Arc<CloneCounterInner>);
impl CloneCounterObserver {
    pub fn current(&self) -> usize {
        self.0.current()
    }
}

impl IntoFuture for CloneCounterObserver {
    type Output = ();

    type IntoFuture = CloneCounterFuture;

    fn into_future(self) -> Self::IntoFuture {
        CloneCounterFuture(self.0)
    }
}

/// A future that waits for the clone counter to decrement to zero
#[derive(Clone, Debug)]
pub struct CloneCounterFuture(Arc<CloneCounterInner>);
impl Future for CloneCounterFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if 0 == self.0.current() {
            Poll::Ready(())
        } else {
            self.0.register(cx);
            if 0 == self.0.current() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }
}
