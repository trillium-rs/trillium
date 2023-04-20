use async_dup::Arc;
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    fmt::{Debug, Display},
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

impl std::io::Write for CloseableCursor {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write().unwrap().data.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
