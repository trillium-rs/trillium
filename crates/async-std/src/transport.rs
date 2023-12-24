use async_std::net::{TcpStream, ToSocketAddrs};
use std::{io::Result, net::SocketAddr};
use trillium_macros::{AsyncRead, AsyncWrite};
use trillium_server_common::{AsyncRead, AsyncWrite, Transport};

/// A transport newtype for async-std
#[derive(Debug, Clone, AsyncRead, AsyncWrite)]
pub struct AsyncStdTransport<T>(T);

impl<T> AsyncStdTransport<T> {
    /// returns the contained type
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl AsyncStdTransport<TcpStream> {
    /// initiates an outbound http connection
    pub async fn connect(socket: impl ToSocketAddrs) -> Result<Self> {
        TcpStream::connect(socket).await.map(Self)
    }
}

impl<T> From<T> for AsyncStdTransport<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl Transport for AsyncStdTransport<TcpStream> {
    fn peer_addr(&self) -> Result<Option<SocketAddr>> {
        self.0.peer_addr().map(Some)
    }

    fn set_ip_ttl(&mut self, ttl: u32) -> Result<()> {
        self.0.set_ttl(ttl)
    }

    fn set_nodelay(&mut self, nodelay: bool) -> Result<()> {
        self.0.set_nodelay(nodelay)
    }
}

#[cfg(unix)]
impl Transport for AsyncStdTransport<async_std::os::unix::net::UnixStream> {}
