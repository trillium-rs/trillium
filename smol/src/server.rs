use async_global_executor::{block_on, spawn, spawn_local};
use async_net::{TcpListener, TcpStream};
use futures_lite::prelude::*;
use std::{
    fs::Metadata,
    future::Future,
    io::Result,
    net::IpAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
use trillium::{Handler, Info, Runtime};
use trillium_server_common::{standard_server, Acceptor, Config, Server, Stopper};

const SERVER_DESCRIPTION: &str = concat!(
    " (",
    env!("CARGO_PKG_NAME"),
    " v",
    env!("CARGO_PKG_VERSION"),
    ")"
);

impl Smol<()> {
    /// constructs a new Smol server with default config and acceptor
    pub fn new() -> Self {
        Self::default()
    }
}

/// The runtime adapter type for smol
#[derive(Debug, Clone)]
pub struct Smol<A> {
    config: Config,
    acceptor: A,
}

impl Default for Smol<()> {
    fn default() -> Self {
        Self {
            config: Config::default(),
            acceptor: (),
        }
    }
}

impl<A> Smol<A> where A: Acceptor<TcpStream> {}

standard_server!(Smol, transport: TcpStream, listener: TcpListener);

impl<A> Runtime for Smol<A>
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
        spawn(future).detach()
    }

    fn spawn_with_handle<F>(future: F) -> Pin<Box<dyn Future<Output = F::Output>>>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        spawn(future).boxed()
    }

    fn spawn_local<F>(future: F)
    where
        F: Future + 'static,
    {
        spawn_local(future).detach()
    }
}

#[trillium::async_trait]
impl<A> trillium::FileSystem for Smol<A> {
    type File = async_fs::File;
    async fn canonicalize<P: AsRef<Path> + Send>(path: P) -> Result<PathBuf> {
        async_fs::canonicalize(path).await
    }

    async fn metadata<P: AsRef<Path> + Send>(path: P) -> Result<Metadata> {
        async_fs::metadata(path).await
    }

    async fn open<P: AsRef<Path> + Send>(path: P) -> Result<Self::File> {
        Self::File::open(path).await
    }
}

#[cfg(unix)]
async fn handle_signals(stop: Stopper) {
    use signal_hook::consts::signal::*;
    use signal_hook_async_std::Signals;

    let signals = Signals::new(&[SIGINT, SIGTERM, SIGQUIT]).unwrap();
    let mut signals = signals.fuse();
    while signals.next().await.is_some() {
        if stop.is_stopped() {
            println!("\nSecond interrupt, shutting down harshly");
            std::process::exit(1);
        } else {
            println!("\nShutting down gracefully.\nControl-C again to force.");
            stop.stop();
        }
    }
}

impl<A: Acceptor<TcpStream>> Server for Smol<A> {
    fn run_async(self, handler: impl Handler<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(Smol::run_async(self, handler))
    }

    #[cfg(unix)]
    fn handle_signals(&self) {
        Self::spawn(handle_signals(self.config.stopper().clone()));
    }
}

impl From<Config> for Smol<()> {
    fn from(config: Config) -> Self {
        Self {
            config,
            acceptor: (),
        }
    }
}
