use crate::Conn;
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

/// Upper bound on bytes buffered from a peer while a handler runs without
/// reading its request. Beyond this, the socket backs up instead of memory.
const PROBE_WINDOW_CAP: usize = 16 * 1024;

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

        // A peer that pipelines faster than handlers consume is put under
        // backpressure rather than buffered without bound; a connection this
        // busy is definitionally alive.
        let room = PROBE_WINDOW_CAP.saturating_sub(buffer.live_len());
        if room == 0 {
            return Poll::Pending;
        }

        match Pin::new(transport).poll_read(cx, buffer.window(room)) {
            Poll::Pending => Poll::Pending,

            Poll::Ready(Err(_)) => Poll::Ready(()),

            Poll::Ready(Ok(n)) => {
                if n == 0 {
                    Poll::Ready(())
                } else {
                    buffer.advance(n);
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
