use async_native_tls::TlsStream;
use std::{
    fmt::Debug,
    io::{IoSlice, IoSliceMut, Result},
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use trillium_server_common::{AsyncRead, AsyncWrite, Transport};

/**
Transport for the native tls connector

This may represent either an encrypted tls connection or a plaintext
connection
*/

#[derive(Debug)]
pub struct NativeTlsTransport<T>(NativeTlsTransportInner<T>);
impl<T> From<T> for NativeTlsTransport<T> {
    fn from(value: T) -> Self {
        Self(Tcp(value))
    }
}

impl<T> From<TlsStream<T>> for NativeTlsTransport<T> {
    fn from(value: TlsStream<T>) -> Self {
        Self(Tls(value))
    }
}

impl<T: Transport> AsRef<T> for NativeTlsTransport<T> {
    fn as_ref(&self) -> &T {
        match &self.0 {
            Tcp(transport) => transport,
            Tls(tls_stream) => tls_stream.get_ref(),
        }
    }
}

#[derive(Debug)]
enum NativeTlsTransportInner<T> {
    Tcp(T),
    Tls(TlsStream<T>),
}
use NativeTlsTransportInner::{Tcp, Tls};

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for NativeTlsTransport<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_read(cx, buf),
            Tls(t) => Pin::new(t).poll_read(cx, buf),
        }
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_read_vectored(cx, bufs),
            Tls(t) => Pin::new(t).poll_read_vectored(cx, bufs),
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncWrite for NativeTlsTransport<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_write(cx, buf),
            Tls(t) => Pin::new(t).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_flush(cx),
            Tls(t) => Pin::new(t).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_close(cx),
            Tls(t) => Pin::new(t).poll_close(cx),
        }
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_write_vectored(cx, bufs),
            Tls(t) => Pin::new(t).poll_write_vectored(cx, bufs),
        }
    }
}

impl<T: Transport> Transport for NativeTlsTransport<T> {
    fn peer_addr(&self) -> Result<Option<SocketAddr>> {
        self.as_ref().peer_addr()
    }
}
