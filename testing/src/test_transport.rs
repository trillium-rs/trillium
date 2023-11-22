use async_dup::Arc;
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    fmt::{Debug, Display},
    future::Future,
    io,
    pin::Pin,
    sync::RwLock,
    task::{Context, Poll, Waker},
};
use trillium_macros::{AsyncRead, AsyncWrite};

/// a readable and writable transport for testing
#[derive(Default, Clone, Debug, AsyncRead, AsyncWrite)]
pub struct TestTransport {
    /// the read side of this transport
    #[async_read]
    pub read: Arc<CloseableCursor>,

    /// the write side of this transport
    #[async_write]
    pub write: Arc<CloseableCursor>,
}

impl trillium_http::transport::Transport for TestTransport {}

impl TestTransport {
    /// constructs a new test transport pair, representing two ends of
    /// a connection. either of them can be written to, and the
    /// content will be readable from the other. either of them can
    /// also be closed.
    pub fn new() -> (TestTransport, TestTransport) {
        let a = Arc::new(CloseableCursor::default());
        let b = Arc::new(CloseableCursor::default());

        (
            TestTransport {
                read: a.clone(),
                write: b.clone(),
            },
            TestTransport { read: b, write: a },
        )
    }

    // pub fn all_read(&self) -> bool {
    //     self.write.current()
    // }

    /// close this transport, representing a disconnection
    pub fn close(&mut self) {
        self.write.close();
    }

    /// take an owned snapshot of the received data
    pub fn snapshot(&self) -> Vec<u8> {
        self.read.snapshot()
    }

    /// synchronously append the supplied bytes to the write side of this transport, notifying the
    /// read side of the other end
    pub fn write_all(&self, bytes: impl AsRef<[u8]>) {
        io::Write::write_all(&mut &*self.write, bytes.as_ref()).unwrap();
    }

    /// waits until there is content and then reads that content to a string until there is no
    /// further content immediately available
    pub async fn read_available(&self) -> Vec<u8> {
        self.read.read_available().await
    }

    ///
    pub async fn read_available_string(&self) -> String {
        self.read.read_available_string().await
    }
}

#[derive(Default)]
struct CloseableCursorInner {
    data: Vec<u8>,
    cursor: usize,
    waker: Option<Waker>,
    closed: bool,
}

#[derive(Default)]
pub struct CloseableCursor(RwLock<CloseableCursorInner>);

impl CloseableCursor {
    /**
    the length of the content
    */
    pub fn len(&self) -> usize {
        self.0.read().unwrap().data.len()
    }

    /**
    the current read position
    */
    pub fn cursor(&self) -> usize {
        self.0.read().unwrap().cursor
    }

    /**
    does what it says on the tin
    */
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// take a snapshot of the data
    pub fn snapshot(&self) -> Vec<u8> {
        self.0.read().unwrap().data.clone()
    }

    /**
    have we read to the end of the available content
    */
    pub fn current(&self) -> bool {
        let inner = self.0.read().unwrap();
        inner.data.len() == inner.cursor
    }

    /**
    close this cursor, waking any pending polls
    */
    pub fn close(&self) {
        let mut inner = self.0.write().unwrap();
        inner.closed = true;
        if let Some(waker) = inner.waker.take() {
            waker.wake();
        }
    }

    /// read any available bytes
    pub async fn read_available(&self) -> Vec<u8> {
        ReadAvailable(self).await.unwrap()
    }

    /// read any available bytes as a string
    pub async fn read_available_string(&self) -> String {
        String::from_utf8(self.read_available().await).unwrap()
    }
}

struct ReadAvailable<T>(T);

impl<T: AsyncRead + Unpin> Future for ReadAvailable<T> {
    type Output = io::Result<Vec<u8>>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut buf = vec![];
        let mut bytes_read = 0;
        loop {
            if buf.len() == bytes_read {
                buf.reserve(32);
                buf.resize(buf.capacity(), 0);
            }
            match Pin::new(&mut self.0).poll_read(cx, &mut buf[bytes_read..]) {
                Poll::Ready(Ok(0)) => break,
                Poll::Ready(Ok(new_bytes)) => {
                    bytes_read += new_bytes;
                }
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending if bytes_read == 0 => return Poll::Pending,
                Poll::Pending => break,
            }
        }

        buf.truncate(bytes_read);
        Poll::Ready(Ok(buf))
    }
}

impl Display for CloseableCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.0.read().unwrap();
        write!(f, "{}", String::from_utf8_lossy(&inner.data))
    }
}

impl Debug for CloseableCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.0.read().unwrap();
        f.debug_struct("CloseableCursor")
            .field(
                "data",
                &std::str::from_utf8(&inner.data).unwrap_or("not utf8"),
            )
            .field("closed", &inner.closed)
            .field("cursor", &inner.cursor)
            .finish()
    }
}

impl AsyncRead for CloseableCursor {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut &*self).poll_read(cx, buf)
    }
}

impl AsyncRead for &CloseableCursor {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut inner = self.0.write().unwrap();
        if inner.cursor < inner.data.len() {
            let bytes_to_copy = buf.len().min(inner.data.len() - inner.cursor);
            buf[..bytes_to_copy]
                .copy_from_slice(&inner.data[inner.cursor..inner.cursor + bytes_to_copy]);
            inner.cursor += bytes_to_copy;
            Poll::Ready(Ok(bytes_to_copy))
        } else if inner.closed {
            Poll::Ready(Ok(0))
        } else {
            inner.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl AsyncWrite for &CloseableCursor {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let mut inner = self.0.write().unwrap();
        if inner.closed {
            Poll::Ready(Ok(0))
        } else {
            inner.data.extend_from_slice(buf);
            if let Some(waker) = inner.waker.take() {
                waker.wake();
            }
            Poll::Ready(Ok(buf.len()))
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.close();
        Poll::Ready(Ok(()))
    }
}

impl io::Write for CloseableCursor {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        io::Write::write(&mut &*self, buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Write for &CloseableCursor {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = self.0.write().unwrap();
        if inner.closed {
            Ok(0)
        } else {
            inner.data.extend_from_slice(buf);
            if let Some(waker) = inner.waker.take() {
                waker.wake();
            }
            Ok(buf.len())
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
