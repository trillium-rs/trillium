use atomic_waker::AtomicWaker;
use futures_lite::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

#[derive(Default)]
pub struct CloneCounterInner {
    count: AtomicUsize,
    waker: AtomicWaker,
}

#[derive(Default)]
pub struct CloneCounter(Arc<CloneCounterInner>);

impl CloneCounter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> usize {
        self.0.count.load(Ordering::SeqCst)
    }
}

impl Future for CloneCounter {
    type Output = ();

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if 0 == self.current() {
            Poll::Ready(())
        } else {
            self.0.waker.register(cx.waker());
            if 0 == self.current() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }
}

impl Clone for CloneCounter {
    fn clone(&self) -> Self {
        self.0.count.fetch_add(1, Ordering::SeqCst);
        Self(self.0.clone())
    }
}
impl Drop for CloneCounter {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::SeqCst);
        self.0.waker.wake();
    }
}
