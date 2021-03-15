use crate::{Conn, Stopper};
use futures_lite::{AsyncRead, AsyncWrite};
use http_types::{headers::Headers, Extensions, Method};
use std::fmt::{self, Debug, Formatter};
use std::pin::Pin;
use std::task::Poll;

pub struct Upgrade<RW> {
    pub request_headers: Headers,
    pub path: String,
    pub method: Method,
    pub state: Extensions,
    pub rw: RW,
    pub buffer: Option<Vec<u8>>,
    pub stopper: Stopper,
}

impl<RW> Upgrade<RW> {
    pub fn headers(&self) -> &Headers {
        &self.request_headers
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn method(&self) -> &Method {
        &self.method
    }
    pub fn state(&self) -> &Extensions {
        &self.state
    }

    pub fn map_transport<T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static>(
        self,
        f: impl Fn(RW) -> T,
    ) -> Upgrade<T> {
        Upgrade {
            rw: f(self.rw),
            path: self.path,
            method: self.method,
            state: self.state,
            buffer: self.buffer,
            request_headers: self.request_headers,
            stopper: self.stopper,
        }
    }
}

impl<RW> Debug for Upgrade<RW> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(&format!("Upgrade<{}>", std::any::type_name::<RW>()))
            .field("request_headers", &self.request_headers)
            .field("path", &self.path)
            .field("method", &self.method)
            .field("buffer", &self.buffer.as_deref().map(utf8))
            .finish()
    }
}

impl<RW> From<Conn<RW>> for Upgrade<RW> {
    fn from(conn: Conn<RW>) -> Self {
        let Conn {
            request_headers,
            path,
            method,
            state,
            rw,
            buffer,
            stopper,
            ..
        } = conn;

        Self {
            request_headers,
            path,
            method,
            state,
            rw,
            buffer,
            stopper,
        }
    }
}
pub fn utf8(d: &[u8]) -> &str {
    std::str::from_utf8(d).unwrap_or("not utf8")
}

impl<RW: AsyncRead + Unpin> AsyncRead for Upgrade<RW> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.buffer.take() {
            Some(mut buffer) if !buffer.is_empty() => {
                let len = buffer.len();
                if len > buf.len() {
                    log::trace!(
                        "have {} bytes of pending data but can only use {}",
                        len,
                        buf.len()
                    );
                    let remaining = buffer.split_off(buf.len());
                    buf.copy_from_slice(&buffer[..]);
                    self.buffer = Some(remaining);
                    Poll::Ready(Ok(buf.len()))
                } else {
                    log::trace!("have {} bytes of pending data, using all of it", len);
                    buf[..len].copy_from_slice(&buffer);
                    self.buffer = None;
                    match Pin::new(&mut self.rw).poll_read(cx, &mut buf[len..]) {
                        Poll::Ready(Ok(e)) => Poll::Ready(Ok(e + len)),
                        Poll::Pending => Poll::Ready(Ok(len)),
                        other => other,
                    }
                }
            }

            _ => Pin::new(&mut self.rw).poll_read(cx, buf),
        }
    }
}

impl<RW: AsyncWrite + Unpin> AsyncWrite for Upgrade<RW> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        Pin::new(&mut self.rw).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.rw).poll_flush(cx)
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        Pin::new(&mut self.rw).poll_close(cx)
    }
}
