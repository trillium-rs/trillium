use super::RuntimeTrait;
use std::{future::Future, thread::JoinHandle};

/// A [`RuntimeTrait`] that can fan a workload out across one OS thread per core, each driving its
/// own single-threaded executor.
///
/// This is the capability behind SO_REUSEPORT thread-per-core deployment: a server binds one
/// listener per worker into a kernel reuseport group and runs each listener's accept loop on the
/// core the kernel delivered the connection to, with no cross-core migration. Runtimes opt into
/// this by implementing the trait; the presence of an impl is what makes
/// `ListenerConfig::bind_reuseport_*` available.
#[doc(hidden)]
pub trait FanOut: RuntimeTrait {
    /// Spawn `count` OS threads, each running `worker(index)` to completion on its own
    /// single-threaded executor, and return their join handles.
    ///
    /// Each thread receives a clone of `worker` and a distinct `index` in `0..count`. Because the
    /// worker future is driven on a current-thread executor it need not be `Send`.
    fn thread_per_core<F, Fut>(&self, count: usize, worker: F) -> Vec<JoinHandle<()>>
    where
        F: Fn(usize) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ()> + 'static;
}
