use crate::async_trait;
use futures_lite::{AsyncRead, AsyncSeek, AsyncWrite};
use std::{
    fs::Metadata,
    future::Future,
    io::Result,
    path::{Path, PathBuf},
    pin::Pin,
};

/// A trait for async runtimes to support spawning futures/tasks
pub trait Runtime: Send + Sync + 'static {
    /// spawn a task on the runtime
    fn spawn<F>(future: F)
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static;

    /// spawn a task on the runtime and return a future that awaits the completion
    fn spawn_with_handle<F>(future: F) -> Pin<Box<dyn Future<Output = F::Output>>>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static;

    /// spawn_local a task on the runtime
    fn spawn_local<F>(future: F)
    where
        F: Future + 'static,
        F::Output: 'static;

    /// block on a task
    fn block_on<F>(future: F) -> F::Output
    where
        Self: Sized,
        F: Future;
}

/// A trait for async runtimes to support simple file operations
#[async_trait]
pub trait FileSystem {
    /**
    The AsyncSeek + AsyncRead + AsyncWrite type for this
    implementation. Note that no operations beyond AsyncRead and
    AsyncWrite are currently supported.
    */
    type File: AsyncSeek + AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static;

    /// an async implementation of [`std::fs::canonicalize`] for this runtime
    async fn canonicalize<P: AsRef<Path> + Send + Sync>(path: P) -> Result<PathBuf>;

    /// an async implementation of [`std::fs::metadata`] for this runtime
    async fn metadata<P: AsRef<Path> + Send + Sync>(path: P) -> Result<Metadata>;

    /// open a file at a given path
    async fn open<P: AsRef<Path> + Send + Sync>(path: P) -> Result<Self::File>;
}
