use async_std::{
    net::{TcpListener, TcpStream},
    prelude::*,
    task::block_on,
};
use std::{
    fs::Metadata,
    io::Result,
    net::IpAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
use trillium::{async_trait, FileSystem, Handler, Info, Runtime};
use trillium_server_common::{Acceptor, Config, Server, Stopper};

const SERVER_DESCRIPTION: &str = concat!(
    " (",
    env!("CARGO_PKG_NAME"),
    " v",
    env!("CARGO_PKG_VERSION"),
    ")"
);

#[cfg(unix)]
async fn handle_signals(stop: Stopper) {
    use signal_hook::consts::signal::*;
    use signal_hook_async_std::Signals;

    let signals = Signals::new(&[SIGINT, SIGTERM, SIGQUIT]).unwrap();
    let mut signals = signals.fuse();
    while signals.next().await.is_some() {
        if stop.is_stopped() {
            println!("second interrupt, shutting down harshly");
            std::process::exit(1);
        } else {
            println!("shutting down gracefully");
            stop.stop();
        }
    }
}

#[derive(Debug, Clone)]
pub struct AsyncStdServer<A> {
    acceptor: A,
    config: Config,
}

impl AsyncStdServer<()> {
    /// constructs a new AsyncStdServer with a default noop [`Acceptor`] and default [`Config`]
    pub fn new() -> Self {
        Self {
            config: Config::default(),
            acceptor: (),
        }
    }
}

impl Default for AsyncStdServer<()> {
    fn default() -> Self {
        Self::new()
    }
}

trillium_server_common::standard_server!(
    AsyncStdServer,
    transport: TcpStream,
    listener: TcpListener
);

impl<A: Acceptor<TcpStream>> Server for AsyncStdServer<A> {
    fn run_async(self, handler: impl Handler<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(AsyncStdServer::run_async(self, handler))
    }

    #[cfg(unix)]
    fn handle_signals(&self) {
        Self::spawn(handle_signals(self.config.stopper().clone()));
    }
}

impl<A> Runtime for AsyncStdServer<A>
where
    A: Send + Sync + 'static,
{
    fn block_on<F>(future: F) -> F::Output
    where
        F: Future,
    {
        block_on(future)
    }

    fn spawn<F>(future: F)
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        async_std::task::spawn(future);
    }

    fn spawn_with_handle<F>(future: F) -> Pin<Box<dyn Future<Output = F::Output>>>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        Box::pin(async_std::task::spawn(future))
    }

    fn spawn_local<F>(future: F)
    where
        F: Future + 'static,
    {
        async_std::task::spawn_local(future);
    }
}

#[async_trait]
impl<A> FileSystem for AsyncStdServer<A> {
    type File = async_std::fs::File;

    async fn canonicalize<P: AsRef<Path> + Send + Sync>(path: P) -> Result<PathBuf> {
        async_std::fs::canonicalize(path.as_ref())
            .await
            .map(Into::into)
    }

    async fn metadata<P: AsRef<Path> + Send + Sync>(path: P) -> Result<Metadata> {
        async_std::fs::metadata(path.as_ref()).await
    }

    async fn open<P: AsRef<Path> + Send + Sync>(path: P) -> Result<Self::File> {
        Self::File::open(path.as_ref()).await
    }
}
