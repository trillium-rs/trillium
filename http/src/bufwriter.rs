use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    fmt,
    io::{Error, ErrorKind, Result},
    pin::Pin,
    task::{ready, Context, Poll},
};
use trillium_macros::AsyncRead;

#[derive(AsyncRead)]
pub(crate) struct BufWriter<W> {
    #[async_read]
    inner: W,
    buf: Vec<u8>,
    written: usize,
}

impl<W: AsyncWrite + Unpin> BufWriter<W> {
    pub(crate) fn new_with_buffer(buf: Vec<u8>, inner: W) -> Self {
        Self {
            inner,
            buf,
            written: 0,
        }
    }

    fn poll_flush_buf(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let Self {
            inner,
            buf,
            written,
        } = &mut *self;

        let len = buf.len();
        let mut ret = Ok(());

        while *written < len {
            let buf = &buf[*written..];
            match ready!(Pin::new(&mut *inner).poll_write(cx, buf)) {
                Ok(0) => {
                    ret = Err(Error::new(
                        ErrorKind::WriteZero,
                        "Failed to write buffered data",
                    ));
                    break;
                }
                Ok(n) => *written += n,
                Err(ref e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => {
                    ret = Err(e);
                    break;
                }
            }
        }

        if *written > 0 {
            buf.drain(..*written);
        }
        *written = 0;

        Poll::Ready(ret)
    }
}

impl<W: fmt::Debug> fmt::Debug for BufWriter<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BufWriter")
            .field("writer", &self.inner)
            .field("buf", &self.buf)
            .field("written", &self.written)
            .finish()
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for BufWriter<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        if self.buf.len() + buf.len() > self.buf.capacity() {
            ready!(self.as_mut().poll_flush_buf(cx))?;
        }
        if buf.len() >= self.buf.capacity() {
            Pin::new(&mut self.inner).poll_write(cx, buf)
        } else {
            self.buf.extend_from_slice(buf);
            Poll::Ready(Ok(buf.len()))
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        ready!(self.as_mut().poll_flush_buf(cx))?;
        Pin::new(&mut self.inner).poll_close(cx)
    }
}
