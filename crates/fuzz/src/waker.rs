//! A minimal `Waker` implementation, to enable manual polling of a future.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    task::{RawWaker, RawWakerVTable, Waker},
};

pub struct WakerInner {
    wake_count: AtomicU64,
}

impl WakerInner {
    fn new() -> Self {
        WakerInner {
            wake_count: AtomicU64::new(0),
        }
    }

    pub fn wake_count(&self) -> u64 {
        self.wake_count.load(Ordering::Relaxed)
    }
}

unsafe fn waker_clone(data: *const ()) -> RawWaker {
    let arc = Arc::from_raw(data as *const WakerInner);
    let _ = Arc::clone(&arc);
    let _ = Arc::into_raw(arc);
    RawWaker::new(data, &VTABLE)
}

unsafe fn waker_wake(data: *const ()) {
    let arc = Arc::from_raw(data as *const WakerInner);
    arc.wake_count.fetch_add(1, Ordering::Relaxed);
    drop(arc);
}

unsafe fn waker_wake_by_ref(data: *const ()) {
    let arc = Arc::from_raw(data as *const WakerInner);
    arc.wake_count.fetch_add(1, Ordering::Relaxed);
    let _ = Arc::into_raw(arc);
}

unsafe fn waker_drop(data: *const ()) {
    let arc = Arc::from_raw(data as *const WakerInner);
    drop(arc);
}

static VTABLE: RawWakerVTable =
    RawWakerVTable::new(waker_clone, waker_wake, waker_wake_by_ref, waker_drop);

/// This constructs a no-op `Waker` that just counts how many times it is called.
pub fn stub_waker() -> (Waker, Arc<WakerInner>) {
    let arc = Arc::new(WakerInner::new());
    let arc_clone = Arc::clone(&arc);
    let ptr = Arc::into_raw(arc);
    let raw_waker = RawWaker::new(ptr as *const (), &VTABLE);
    let waker = unsafe { Waker::from_raw(raw_waker) };
    (waker, arc_clone)
}
