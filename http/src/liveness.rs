use crate::Conn;
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

pub(crate) struct LivenessFut<'a, T>(&'a mut Conn<T>);

impl<'a, T> LivenessFut<'a, T> {
    pub(crate) fn new(conn: &'a mut Conn<T>) -> Self {
        Self(conn)
    }
}

impl<T> Future for LivenessFut<'_, T>
where
    T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let LivenessFut(Conn {
            buffer, transport, ..
        }) = &mut *self;

        let len = buffer.len();
        buffer.expand();
        match Pin::new(transport).poll_read(cx, &mut buffer[len..]) {
            Poll::Pending => {
                buffer.truncate(len);
                Poll::Pending
            }

            Poll::Ready(Err(_)) => {
                buffer.truncate(len);
                Poll::Ready(())
            }

            Poll::Ready(Ok(n)) => {
                buffer.truncate(len + n);
                if n == 0 {
                    Poll::Ready(())
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        }
    }
}

pub(crate) struct CancelOnDisconnect<'a, Fut, T>(
    pub(crate) &'a mut Conn<T>,
    pub(crate) Pin<&'a mut Fut>,
);
impl<'a, Fut, T> Future for CancelOnDisconnect<'a, Fut, T>
where
    Fut: Future + Send + 'a,
    T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    type Output = Option<Fut::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let CancelOnDisconnect(conn, fut) = &mut *self;
        let fut_poll = fut.as_mut().poll(cx);
        let disconnect = Pin::new(&mut LivenessFut(conn)).poll(cx);
        match (fut_poll, disconnect) {
            (Poll::Ready(output), _) => Poll::Ready(Some(output)),
            (Poll::Pending, Poll::Ready(())) => Poll::Ready(None),
            (Poll::Pending, Poll::Pending) => Poll::Pending,
        }
    }
}
