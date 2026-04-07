use futures_lite::{AsyncRead, AsyncWrite};
use std::{any::Any, io::Result, net::SocketAddr, time::Duration};

/// # The interface that the http protocol is communicated over.
///
/// This trait supports several common interfaces supported by tcp
/// streams, but also can be implemented for other stream types. All trait
/// functions are currently optional.
///
/// **Note:** `Transport` uses the [`AsyncWrite`](futures_lite::AsyncWrite) and
/// [`AsyncRead`](futures_lite::AsyncRead) traits from [`futures-lite`](futures_lite), which differ
/// from the tokio `AsyncRead` / `AsyncWrite` traits. Runtime adapters handle bridging these at the
/// boundary.
#[allow(unused)]
pub trait Transport: Any + AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static {
    /// # Sets the linger duration of this transport by setting the `SO_LINGER` option
    ///
    /// See [`std::net::TcpStream::set_linger`]
    /// Optional to implement.
    ///
    /// # Errors
    ///
    /// Return an error if this transport supports setting linger and
    /// attempting to do so is unsuccessful.
    fn set_linger(&mut self, linger: Option<Duration>) -> Result<()> {
        Ok(())
    }

    /// # Sets the value of the `TCP_NODELAY` option on this transport.
    ///
    /// See [`std::net::TcpStream::set_nodelay`].
    /// Optional to implement.
    ///
    /// # Errors
    ///
    /// Return an error if this transport supports setting nodelay and
    /// attempting to do so is unsuccessful.
    fn set_nodelay(&mut self, nodelay: bool) -> Result<()> {
        Ok(())
    }

    /// # Sets the value for the `IP_TTL` option on this transport.
    ///
    /// See [`std::net::TcpStream::set_ttl`]
    /// Optional to implement
    ///
    /// # Errors
    ///
    /// Return an error if this transport supports setting ttl and
    /// attempting to do so is unsuccessful.
    fn set_ip_ttl(&mut self, ttl: u32) -> Result<()> {
        Ok(())
    }

    /// # Returns the socket address of the remote peer of this transport.
    ///
    /// # Errors
    ///
    /// Return an error if this transport supports retrieving the
    /// remote peer but attempting to do so is unsuccessful.
    fn peer_addr(&self) -> Result<Option<SocketAddr>> {
        Ok(None)
    }
}

impl Transport for Box<dyn Transport> {
    fn set_linger(&mut self, linger: Option<Duration>) -> Result<()> {
        (**self).set_linger(linger)
    }

    fn set_nodelay(&mut self, nodelay: bool) -> Result<()> {
        (**self).set_nodelay(nodelay)
    }

    fn set_ip_ttl(&mut self, ttl: u32) -> Result<()> {
        (**self).set_ip_ttl(ttl)
    }

    fn peer_addr(&self) -> Result<Option<SocketAddr>> {
        (**self).peer_addr()
    }
}

impl Transport for trillium_http::Synthetic {}
