use crate::Transport;
use futures_lite::{AsyncRead, AsyncWrite, Stream};
use std::{
    io::{IoSlice, Result},
    pin::Pin,
    task::{Context, Poll},
};

/// A wrapper enum that has blanket implementations for common traits
/// like TryFrom, Stream, AsyncRead, and AsyncWrite. This can contain
/// listeners (like TcpListener), Streams (like Incoming), or
/// bytestreams (like TcpStream).
#[derive(Debug, Clone)]
pub enum Binding<T, U> {
    /// a tcp type (listener or incoming or stream)
    Tcp(T),

    /// a unix type (listener or incoming or stream)
    Unix(U),
}

use Binding::{Tcp, Unix};

impl<T, U> Binding<T, U> {
    /// borrows the tcp stream or listener, if this is a tcp variant
    pub fn get_tcp(&self) -> Option<&T> {
        if let Tcp(t) = self {
            Some(t)
        } else {
            None
        }
    }

    /// borrows the unix stream or listener, if this is unix variant
    pub fn get_unix(&self) -> Option<&U> {
        if let Unix(u) = self {
            Some(u)
        } else {
            None
        }
    }

    /// mutably borrows the tcp stream or listener, if this is tcp variant
    pub fn get_tcp_mut(&mut self) -> Option<&mut T> {
        if let Tcp(t) = self {
            Some(t)
        } else {
            None
        }
    }

    /// mutably borrows the unix stream or listener, if this is unix variant
    pub fn get_unix_mut(&mut self) -> Option<&mut U> {
        if let Unix(u) = self {
            Some(u)
        } else {
            None
        }
    }
}

impl<T: TryFrom<std::net::TcpListener>, U> TryFrom<std::net::TcpListener> for Binding<T, U> {
    type Error = <T as TryFrom<std::net::TcpListener>>::Error;

    fn try_from(value: std::net::TcpListener) -> std::result::Result<Self, Self::Error> {
        Ok(Self::Tcp(value.try_into()?))
    }
}

#[cfg(unix)]
impl<T, U: TryFrom<std::os::unix::net::UnixListener>> TryFrom<std::os::unix::net::UnixListener>
    for Binding<T, U>
{
    type Error = <U as TryFrom<std::os::unix::net::UnixListener>>::Error;

    fn try_from(value: std::os::unix::net::UnixListener) -> std::result::Result<Self, Self::Error> {
        Ok(Self::Unix(value.try_into()?))
    }
}

impl<T, U, TI, UI> Stream for Binding<T, U>
where
    T: Stream<Item = Result<TI>> + Unpin,
    U: Stream<Item = Result<UI>> + Unpin,
{
    type Item = Result<Binding<TI, UI>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match &mut *self {
            Tcp(t) => Pin::new(t).poll_next(cx).map(|i| i.map(|x| x.map(Tcp))),
            Unix(u) => Pin::new(u).poll_next(cx).map(|i| i.map(|x| x.map(Unix))),
        }
    }
}

impl<T, U> Binding<T, U>
where
    T: AsyncRead + Unpin,
    U: AsyncRead + Unpin,
{
    fn as_async_read(&mut self) -> Pin<&mut (dyn AsyncRead + Unpin)> {
        Pin::new(match self {
            Tcp(t) => t as &mut (dyn AsyncRead + Unpin),
            Unix(u) => u as &mut (dyn AsyncRead + Unpin),
        })
    }
}

impl<T, U> Binding<T, U>
where
    T: AsyncWrite + Unpin,
    U: AsyncWrite + Unpin,
{
    fn as_async_write(&mut self) -> Pin<&mut (dyn AsyncWrite + Unpin)> {
        Pin::new(match self {
            Tcp(t) => t as &mut (dyn AsyncWrite + Unpin),
            Unix(u) => u as &mut (dyn AsyncWrite + Unpin),
        })
    }
}

impl<T, U> AsyncRead for Binding<T, U>
where
    T: AsyncRead + Unpin,
    U: AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        self.as_async_read().poll_read(cx, buf)
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [std::io::IoSliceMut<'_>],
    ) -> Poll<Result<usize>> {
        self.as_async_read().poll_read_vectored(cx, bufs)
    }
}

impl<T, U> AsyncWrite for Binding<T, U>
where
    T: AsyncWrite + Unpin,
    U: AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        self.as_async_write().poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.as_async_write().poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.as_async_write().poll_close(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize>> {
        self.as_async_write().poll_write_vectored(cx, bufs)
    }
}

impl<T, U> Binding<T, U>
where
    T: Transport,
    U: Transport,
{
    fn as_transport_mut(&mut self) -> &mut dyn Transport {
        match self {
            Tcp(t) => t as &mut dyn Transport,
            Unix(u) => u as &mut dyn Transport,
        }
    }

    fn as_transport(&self) -> &dyn Transport {
        match self {
            Tcp(t) => t as &dyn Transport,
            Unix(u) => u as &dyn Transport,
        }
    }
}

impl<T, U> Transport for Binding<T, U>
where
    T: Transport,
    U: Transport,
{
    fn set_linger(&mut self, linger: Option<std::time::Duration>) -> Result<()> {
        self.as_transport_mut().set_linger(linger)
    }

    fn set_nodelay(&mut self, nodelay: bool) -> Result<()> {
        self.as_transport_mut().set_nodelay(nodelay)
    }

    fn set_ip_ttl(&mut self, ttl: u32) -> Result<()> {
        self.as_transport_mut().set_ip_ttl(ttl)
    }

    fn peer_addr(&self) -> Result<Option<std::net::SocketAddr>> {
        self.as_transport().peer_addr()
    }
}
