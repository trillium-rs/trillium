use super::Proxy;
use crate::{
    Headers, KnownHeaderName, Status,
    conn::{request_head, response_head},
};
use futures_lite::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use std::{
    borrow::Cow,
    fmt,
    io::{self, ErrorKind},
    pin::Pin,
    task::{Context, Poll},
};
use trillium_http::{Buffer, Error, HttpConfig, Result};
use trillium_macros::{AsyncWrite, Transport};
use trillium_server_common::{Connector, Destination, Transport, url};

/// An error returned when an HTTP proxy rejects a `CONNECT` request.
///
/// Carried inside [`Error::Io`](crate::Error::Io) with [`io::ErrorKind::ConnectionRefused`]
/// and recoverable via [`io::Error::get_ref`].
#[derive(Clone, Debug)]
pub struct ProxyConnectError {
    code: u16,
    headers: Headers,
}

impl ProxyConnectError {
    /// The status returned by the proxy for the `CONNECT` request.
    ///
    /// A code that [`Status`] doesn't represent is reported as the `x00` status of its class, or
    /// as [`Status::InternalServerError`] when it is outside 100–599.
    #[must_use]
    pub fn status(&self) -> Status {
        Status::from_code_lossy(self.code)
    }

    /// The response headers returned by the proxy for the `CONNECT` request.
    #[must_use]
    pub const fn headers(&self) -> &Headers {
        &self.headers
    }
}

impl fmt::Display for ProxyConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = self.status();
        if status == self.code {
            write!(f, "CONNECT proxy returned {status}")
        } else {
            write!(
                f,
                "CONNECT proxy returned {status} (status code {})",
                self.code
            )
        }
    }
}

impl std::error::Error for ProxyConnectError {}

#[derive(Debug, AsyncWrite, Transport)]
pub(crate) struct Tunnel<T = Box<dyn Transport>> {
    #[async_write]
    #[transport(except = negotiated_alpn)]
    transport: T,
    /// Bytes that arrived after the proxy's response head. They belong to the tunneled stream, so
    /// they are read before anything further from `transport`.
    leftover: Buffer,
}

impl<T: AsyncRead + Unpin> AsyncRead for Tunnel<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        if this.leftover.is_empty() {
            return Pin::new(&mut this.transport).poll_read(cx, buf);
        }
        let len = buf.len().min(this.leftover.live_len());
        buf[..len].copy_from_slice(&this.leftover[..len]);
        this.leftover.ignore_front(len);
        Poll::Ready(Ok(len))
    }
}

impl<T> Tunnel<T> {
    /// Any ALPN on `transport` was negotiated with the proxy, not with the tunneled origin.
    const fn negotiated_alpn(&self) -> Option<Cow<'_, [u8]>> {
        None
    }
}

pub(crate) async fn open_tunnel<C: Connector + ?Sized>(
    connector: &C,
    proxy: &Proxy,
    origin: &Destination,
    config: &HttpConfig,
) -> Result<Box<dyn Transport>> {
    let request = build_connect_request(proxy, origin)?;
    let mut transport = connector.connect_to(proxy.destination()?).await?;
    let leftover = handshake(&mut transport, &request, config).await?;
    Ok(Box::new(Tunnel {
        transport,
        leftover,
    }))
}

/// The client writes `Host` itself, and a `CONNECT` request has no body and upgrades nothing.
const RESERVED_CONNECT_HEADERS: [KnownHeaderName; 5] = [
    KnownHeaderName::Host,
    KnownHeaderName::ContentLength,
    KnownHeaderName::TransferEncoding,
    KnownHeaderName::Connection,
    KnownHeaderName::Upgrade,
];

fn build_connect_request(proxy: &Proxy, origin: &Destination) -> Result<Vec<u8>> {
    let authority = if let Some(host) = origin.host() {
        let host =
            url::Host::parse(host).map_err(|e| io::Error::new(ErrorKind::InvalidInput, e))?;
        format!("{host}:{}", origin.port())
    } else {
        origin
            .addrs()
            .first()
            .ok_or_else(|| {
                io::Error::new(ErrorKind::InvalidInput, "missing tunnel origin address")
            })?
            .to_string()
    };
    if let Some((name, _)) = proxy.connect_headers().iter().find(|(name, _)| {
        RESERVED_CONNECT_HEADERS
            .iter()
            .any(|reserved| name == *reserved)
    }) {
        return Err(Error::UnexpectedHeader(name.into_owned()));
    }
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n").into_bytes();
    request_head::write_headers(&mut request, proxy.connect_headers())?;
    request.extend_from_slice(b"\r\n");
    Ok(request)
}

/// Sends the CONNECT request and reads the proxy's response, returning any bytes that arrived
/// after the final response head.
async fn handshake<T>(transport: &mut T, request: &[u8], config: &HttpConfig) -> Result<Buffer>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    transport.write_all(request).await?;
    transport.flush().await?;
    let mut buffer = Buffer::default();
    // `read_head` bounds each head; bounding them together also ends an endless run of interim
    // responses.
    let mut consumed = 0;
    loop {
        let head_len = response_head::read_head(transport, &mut buffer, config).await?;
        consumed += head_len;
        if consumed > config.head_max_len() {
            return Err(Error::HeadersTooLong);
        }
        let head = response_head::parse_head(&buffer[..head_len])?;
        buffer.ignore_front(head_len);
        match head.code {
            200..=299 => return Ok(buffer),
            100..=199 if head.code != 101 => {}
            code => {
                let refusal = ProxyConnectError {
                    code,
                    headers: head.headers,
                };
                return Err(io::Error::new(ErrorKind::ConnectionRefused, refusal).into());
            }
        }
    }
}

#[cfg(test)]
mod tests;
