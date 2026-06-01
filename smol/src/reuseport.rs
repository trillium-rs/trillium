use crate::{SmolRuntime, runtime::block_on_worker};
use std::{
    future::Future,
    thread::{self, JoinHandle},
};
use trillium_server_common::FanOut;

impl FanOut for SmolRuntime {
    fn thread_per_core<F, Fut>(&self, count: usize, worker: F) -> Vec<JoinHandle<()>>
    where
        F: Fn(usize) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ()> + 'static,
    {
        (0..count)
            .map(|idx| {
                let worker = worker.clone();
                thread::Builder::new()
                    .name(format!("trillium-reuseport-{idx}"))
                    .spawn(move || block_on_worker(worker(idx)))
                    .expect("could not spawn worker thread")
            })
            .collect()
    }
}
