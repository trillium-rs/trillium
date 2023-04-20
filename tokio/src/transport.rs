use async_compat::Compat;
use std::{io::Result, net::SocketAddr};
use tokio::net::{TcpStream, ToSocketAddrs};
use trillium_macros::{AsyncRead, AsyncWrite};
use trillium_server_common::{AsyncRead, AsyncWrite, Transport};

/// A transport newtype for tokio
#[derive(Debug, Clone, AsyncRead, AsyncWrite)]
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
