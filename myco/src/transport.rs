use futures_lite::io::{AsyncRead, AsyncWrite};
use std::fmt::Debug;
use std::{
    fmt,
    io::Result,
    pin::Pin,
    task::{Context, Poll},
};
pub struct BoxedTransport {
    inner: Box<dyn Transport + Send + Sync + 'static>,
}

impl Debug for BoxedTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoxedTransport")
            .field(
                "inner",
                &"Box<dyn AsyncRead + AsyncWrite + Send + Sync + Unpin>",
            )
            .finish()
    }
}

impl BoxedTransport {
    pub fn new<T>(t: T) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
    {
        Self { inner: Box::new(t) }
    }
}

pub trait Transport: AsyncRead + AsyncWrite + Send + Sync + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Sync + Unpin> Transport for T {}

impl AsyncRead for BoxedTransport {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for BoxedTransport {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}
