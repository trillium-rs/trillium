use crate::TokioRuntime;
use std::{
    future::Future,
    thread::{self, JoinHandle},
};
use tokio::runtime::Builder;
use trillium_server_common::FanOut;

impl FanOut for TokioRuntime {
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
                    .spawn(move || {
                        Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("could not build current-thread runtime")
                            .block_on(worker(idx));
                    })
                    .expect("could not spawn worker thread")
            })
            .collect()
    }
}
