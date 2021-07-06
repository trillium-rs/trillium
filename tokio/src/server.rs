use async_compat::Compat;
use std::{
    fs::Metadata,
    future::Future,
    io::Result,
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};
use tokio::net::{TcpListener, TcpStream};
use tokio_stream::{wrappers::TcpListenerStream, Stream, StreamExt};
use trillium::{Handler, Info, Runtime};
use trillium_server_common::{
    standard_server, Acceptor, AsyncRead, AsyncWrite, Config, Server, Stopper,
};

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
    use signal_hook_tokio::Signals;
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

/// The tokio trillium runtime adapter
#[derive(Debug, Clone)]
pub struct TokioServer<A> {
    config: Config,
    acceptor: A,
}

impl Default for TokioServer<()> {
    fn default() -> Self {
        Self {
            config: Config::default(),
            acceptor: (),
        }
    }
}
impl TokioServer<()> {
    /// build a new tokio server with no tls acceptor
    pub fn new() -> Self {
        Self::default()
    }
}

standard_server!(
    TokioServer,
    transport: StreamAdapter,
    listener: TcpListenerAdapter
);

impl<A: Acceptor<StreamAdapter>> Server for TokioServer<A> {
    #[cfg(unix)]
    fn handle_signals(&self) {
        Self::spawn(handle_signals(self.config.stopper().clone()));
    }

    fn run_async(self, handler: impl Handler<Self>) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(TokioServer::run_async(self, handler))
    }
}

/// A tokio TcpStream adapter for AsyncRead and AsyncWrite
#[allow(missing_debug_implementations)]
pub struct StreamAdapter(Compat<TcpStream>);

impl StreamAdapter {
    pub fn new(stream: TcpStream) -> Self {
        Self(Compat::new(stream))
    }

    pub fn set_nodelay(&self, nodelay: bool) -> Result<()> {
        self.0.get_ref().set_nodelay(nodelay)
    }

    pub fn peer_addr(&self) -> Result<SocketAddr> {
        self.0.get_ref().peer_addr()
    }
}

impl AsyncRead for StreamAdapter {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for StreamAdapter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<()>> {
        Pin::new(&mut self.0).poll_close(cx)
    }
}

/// A tokio TcpListener adapter
struct TcpListenerAdapter(TcpListener);
impl TcpListenerAdapter {
    pub fn incoming(self) -> impl Stream<Item = Result<StreamAdapter>> {
        TcpListenerStream::new(self.0).map(|r| r.map(StreamAdapter::new))
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.0.local_addr()
    }
}

impl TryFrom<std::net::TcpListener> for TcpListenerAdapter {
    type Error = <TcpListener as TryFrom<std::net::TcpListener>>::Error;

    fn try_from(value: std::net::TcpListener) -> std::result::Result<Self, Self::Error> {
        Ok(Self(value.try_into()?))
    }
}

impl<A> Runtime for TokioServer<A>
where
    A: Send + Sync + 'static,
{
    fn block_on<F>(future: F) -> F::Output
    where
        F: Future,
    {
        tokio::runtime::Runtime::new().unwrap().block_on(future)
    }

    fn spawn<F>(future: F)
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        tokio::task::spawn(future);
    }

    fn spawn_with_handle<F>(future: F) -> Pin<Box<dyn Future<Output = F::Output>>>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        Box::pin(async { tokio::task::spawn(future).await.unwrap() })
    }

    fn spawn_local<F>(future: F)
    where
        F: Future + 'static,
    {
        tokio::task::spawn_local(future);
    }
}

#[trillium::async_trait]
impl<A> trillium::FileSystem for TokioServer<A> {
    type File = Compat<tokio::fs::File>;

    async fn canonicalize<P: AsRef<Path> + Send>(path: P) -> Result<PathBuf> {
        tokio::fs::canonicalize(path).await
    }

    async fn metadata<P: AsRef<Path> + Send>(path: P) -> Result<Metadata> {
        tokio::fs::metadata(path).await
    }

    async fn open<P: AsRef<Path> + Send>(path: P) -> Result<Self::File> {
        tokio::fs::File::open(path).await.map(Compat::new)
    }
}
