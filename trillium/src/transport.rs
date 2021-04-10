use futures_lite::io::{AsyncRead, AsyncWrite};
use std::any::Any;
use std::fmt::Debug;
use std::ops::Deref;
use std::{
    fmt,
    io::Result,
    pin::Pin,
    task::{Context, Poll},
};
pub struct BoxedTransport(Box<dyn Transport + Send + Sync + 'static>);

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
        Self(Box::new(t))
    }

    pub fn downcast<T: 'static>(self) -> Option<Box<T>> {
        let inner: Box<dyn Any> = self.0.as_box_any();
        inner.downcast().ok()
    }
}

impl Deref for BoxedTransport {
    type Target = Box<dyn Transport + Send + Sync + 'static>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub trait Transport: Any + AsyncRead + AsyncWrite + Send + Sync + Unpin {
    fn as_box_any(self: Box<Self>) -> Box<dyn Any>;
}
impl<T> Transport for T
where
    T: Any + AsyncRead + AsyncWrite + Send + Sync + Unpin,
{
    fn as_box_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl AsyncRead for BoxedTransport {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for BoxedTransport {
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
