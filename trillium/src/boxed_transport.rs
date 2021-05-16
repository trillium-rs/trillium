use crate::Transport;
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

/**
BoxedTransport represents a `Box<dyn Transport>` that supports
downcasting to the original Transport. This is used in trillium to
erase the generic on Conn, in order to avoid writing `Conn<TcpStream>`
throughout an application.

**Stability Note:** This may go away at some point and be
replaced by a type alias exported from a
`trillium_server_common::Server` crate.
*/
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
    /**
    Create a new BoxedTransport from some Transport.

    ```
    use trillium::BoxedTransport;
    use trillium_testing::TestTransport;
    // this examples uses trillium_testing::TestTransport as an
    // easily-constructed AsyncRead+AsyncWrite.
    let (test_transport, _) = TestTransport::new();

    let boxed = BoxedTransport::new(test_transport);

    let downcast: Option<Box<TestTransport>> = boxed.downcast();
    assert!(downcast.is_some());

    let boxed_again = BoxedTransport::new(*downcast.unwrap());
    assert!(boxed_again.downcast::<async_net::TcpStream>().is_none());
    ```
    */

    pub fn new(t: impl Transport + 'static) -> Self {
        Self(Box::new(t))
    }

    /**
    Attempt to convert the trait object into a specific transport
    T. This will only succeed if T is the type that was originally
    passed to [`BoxedTransport::new`], and will return None otherwise

    see [BoxedTransport::new] for example usage
    */
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
