use async_compat::Compat;
use std::{
    io::Result,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::net::{TcpStream, ToSocketAddrs};
use trillium_server_common::{AsyncRead, AsyncWrite, Transport};

/// A transport newtype for tokio
#[derive(Debug, Clone)]
pub struct TokioTransport<T>(pub(crate) T);

impl<T> TokioTransport<T> {
    /// returns the contained type
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl TokioTransport<Compat<TcpStream>> {
    /// initiates an outbound http connection
    pub async fn connect(socket: impl ToSocketAddrs) -> Result<Self> {
        TcpStream::connect(socket)
            .await
            .map(|t| Self(Compat::new(t)))
    }
}

impl<T> From<T> for TokioTransport<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T> AsyncRead for TokioTransport<T>
where
    T: Unpin + AsyncRead,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl<T> AsyncWrite for TokioTransport<T>
where
    T: Unpin + AsyncWrite,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.0).poll_close(cx)
    }
}

impl Transport for TokioTransport<Compat<TcpStream>> {
    fn peer_addr(&self) -> Result<Option<SocketAddr>> {
        self.0.get_ref().peer_addr().map(Some)
    }

    fn set_ip_ttl(&mut self, ttl: u32) -> Result<()> {
        self.0.get_mut().set_ttl(ttl)
    }

    fn set_nodelay(&mut self, nodelay: bool) -> Result<()> {
        self.0.get_mut().set_nodelay(nodelay)
    }
}

#[cfg(unix)]
impl Transport for TokioTransport<Compat<tokio::net::UnixStream>> {}
