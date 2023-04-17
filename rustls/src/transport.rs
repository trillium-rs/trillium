use async_rustls::{client, server, TlsStream};
use std::{
    fmt::Debug,
    io::Result,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use trillium_server_common::{AsyncRead, AsyncWrite, Transport};
use RustlsTransportInner::{Tcp, Tls};

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
enum RustlsTransportInner<T> {
    Tcp(T),
    Tls(TlsStream<T>),
}

/**
Transport for the rustls connector

This may represent either an encrypted tls connection or a plaintext
connection, depending on the request schema
*/
#[derive(Debug)]
pub struct RustlsTransport<T>(RustlsTransportInner<T>);
impl<T> From<T> for RustlsTransport<T> {
    fn from(value: T) -> Self {
        Self(Tcp(value))
    }
}

impl<T> From<TlsStream<T>> for RustlsTransport<T> {
    fn from(value: TlsStream<T>) -> Self {
        Self(Tls(value))
    }
}

impl<T> From<client::TlsStream<T>> for RustlsTransport<T> {
    fn from(value: client::TlsStream<T>) -> Self {
        TlsStream::from(value).into()
    }
}

impl<T> From<server::TlsStream<T>> for RustlsTransport<T> {
    fn from(value: server::TlsStream<T>) -> Self {
        TlsStream::from(value).into()
    }
}

impl<C> AsyncRead for RustlsTransport<C>
where
    C: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(c) => Pin::new(c).poll_read(cx, buf),
            Tls(c) => Pin::new(c).poll_read(cx, buf),
        }
    }
}

impl<C> AsyncWrite for RustlsTransport<C>
where
    C: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(c) => Pin::new(c).poll_write(cx, buf),
            Tls(c) => Pin::new(c).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut self.0 {
            Tcp(c) => Pin::new(c).poll_flush(cx),
            Tls(c) => Pin::new(c).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut self.0 {
            Tcp(c) => Pin::new(c).poll_close(cx),
            Tls(c) => Pin::new(c).poll_close(cx),
        }
    }
}

impl<T: Transport> Transport for RustlsTransport<T> {
    fn peer_addr(&self) -> Result<Option<SocketAddr>> {
        self.as_ref().peer_addr()
    }
}

impl<T> AsRef<T> for RustlsTransport<T> {
    fn as_ref(&self) -> &T {
        match &self.0 {
            Tcp(x) => x,
            Tls(x) => x.get_ref().0,
        }
    }
}
