use futures_lite::{AsyncRead, AsyncWrite, Stream};
use std::{
    convert::{TryFrom, TryInto},
    io::Result,
    pin::Pin,
    task::{Context, Poll},
};

/// A wrapper enum that has blanket implementations for common traits
/// like TryFrom, Stream, AsyncRead, and AsyncWrite. This can contain
/// listeners (like TcpListener), Streams (like Incoming), or
/// bytestreams (like TcpStream).
#[derive(Debug)]
pub enum Binding<T, U> {
    /// a tcp type (listener or incoming or stream)
    Tcp(T),

    /// a unix type (listener or incoming or stream)
    Unix(U),
}

impl<T: TryFrom<std::net::TcpListener>, U> TryFrom<std::net::TcpListener> for Binding<T, U> {
    type Error = <T as TryFrom<std::net::TcpListener>>::Error;

    fn try_from(value: std::net::TcpListener) -> std::result::Result<Self, Self::Error> {
        Ok(Self::Tcp(value.try_into()?))
    }
}

#[cfg(unix)]
impl<T, U: TryFrom<std::os::unix::net::UnixListener>> TryFrom<std::os::unix::net::UnixListener>
    for Binding<T, U>
{
    type Error = <U as TryFrom<std::os::unix::net::UnixListener>>::Error;

    fn try_from(value: std::os::unix::net::UnixListener) -> std::result::Result<Self, Self::Error> {
        Ok(Self::Unix(value.try_into()?))
    }
}

impl<T, U, TI, UI> Stream for Binding<T, U>
where
    T: Stream<Item = Result<TI>> + Unpin,
    U: Stream<Item = Result<UI>> + Unpin,
{
    type Item = Result<Binding<TI, UI>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match &mut *self {
            Binding::Tcp(t) => Pin::new(t)
                .poll_next(cx)
                .map(|i| i.map(|x| x.map(Binding::Tcp))),

            Binding::Unix(u) => Pin::new(u)
                .poll_next(cx)
                .map(|i| i.map(|x| x.map(Binding::Unix))),
        }
    }
}

impl<T, U> AsyncRead for Binding<T, U>
where
    T: AsyncRead + Unpin,
    U: AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        match &mut *self {
            Binding::Tcp(t) => Pin::new(t).poll_read(cx, buf),
            Binding::Unix(u) => Pin::new(u).poll_read(cx, buf),
        }
    }
}

impl<T, U> AsyncWrite for Binding<T, U>
where
    T: AsyncWrite + Unpin,
    U: AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        match &mut *self {
            Binding::Tcp(t) => Pin::new(t).poll_write(cx, buf),
            Binding::Unix(u) => Pin::new(u).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut *self {
            Binding::Tcp(t) => Pin::new(t).poll_flush(cx),
            Binding::Unix(u) => Pin::new(u).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut *self {
            Binding::Tcp(t) => Pin::new(t).poll_close(cx),
            Binding::Unix(u) => Pin::new(u).poll_close(cx),
        }
    }
}
