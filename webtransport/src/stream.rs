use std::{
    io,
    ops::Deref,
    pin::Pin,
    task::{Context, Poll},
};
use trillium_http::transport::BoxedTransport;
use trillium_macros::{AsyncRead, AsyncWrite};
use trillium_server_common::{AsyncRead, AsyncWrite};

/// A received WebTransport datagram.
///
/// Derefs to `&[u8]` and converts `Into<Vec<u8>>`. The internal representation
/// is not part of the public API and may change in future versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram(Vec<u8>);

impl Deref for Datagram {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for Datagram {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for Datagram {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

impl From<Datagram> for Vec<u8> {
    fn from(d: Datagram) -> Self {
        d.0
    }
}

/// An inbound WebTransport stream, yielded by [`WebTransportConnection::accept_next_stream`].
///
/// Datagrams are handled separately via [`WebTransportConnection::recv_datagram`], as they
/// typically require a dedicated low-latency loop rather than sharing one with stream acceptance.
#[derive(Debug)]
pub enum InboundStream {
    /// An inbound bidirectional stream opened by the client.
    Bidi(InboundBidiStream),
    /// An inbound unidirectional stream opened by the client.
    Uni(InboundUniStream),
}

pub(crate) type BoxedRecvStream = Box<dyn AsyncRead + Unpin + Send + Sync>;
type BoxedSendStream = Box<dyn AsyncWrite + Unpin + Send + Sync>;

/// An inbound bidirectional WebTransport stream opened by the client.
///
/// Implements [`AsyncRead`] and [`AsyncWrite`].
#[derive(AsyncWrite, Debug)]
pub struct InboundBidiStream {
    buffer: Vec<u8>,
    offset: usize,
    #[async_write]
    transport: BoxedTransport,
}

impl InboundBidiStream {
    pub(crate) fn new(transport: BoxedTransport, buffer: Vec<u8>) -> Self {
        Self {
            buffer,
            offset: 0,
            transport,
        }
    }
}

impl AsyncRead for InboundBidiStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        read_buffered(
            &mut this.buffer,
            &mut this.offset,
            &mut this.transport,
            cx,
            buf,
        )
    }
}

/// An inbound unidirectional WebTransport stream opened by the client.
///
/// Implements [`AsyncRead`].
pub struct InboundUniStream {
    buffer: Vec<u8>,
    offset: usize,
    stream: BoxedRecvStream,
}

impl std::fmt::Debug for InboundUniStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboundUniStream")
            .field("buffer", &self.buffer)
            .field("offset", &self.offset)
            .finish_non_exhaustive()
    }
}

impl InboundUniStream {
    pub(crate) fn new(stream: BoxedRecvStream, buffer: Vec<u8>) -> Self {
        Self {
            buffer,
            offset: 0,
            stream,
        }
    }
}

impl AsyncRead for InboundUniStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = &mut *self;
        read_buffered(
            &mut this.buffer,
            &mut this.offset,
            &mut this.stream,
            cx,
            buf,
        )
    }
}

/// A server-initiated bidirectional WebTransport stream.
///
/// Implements [`AsyncRead`] and [`AsyncWrite`].
#[derive(AsyncRead, AsyncWrite)]
pub struct OutboundBidiStream(#[async_io] BoxedTransport);

impl std::fmt::Debug for OutboundBidiStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("OutboundBidiStream").finish_non_exhaustive()
    }
}

impl OutboundBidiStream {
    pub(crate) fn new(transport: BoxedTransport) -> Self {
        Self(transport)
    }
}

/// A server-initiated unidirectional WebTransport stream.
///
/// Implements [`AsyncWrite`].
#[derive(AsyncWrite)]
pub struct OutboundUniStream(#[async_write] BoxedSendStream);

impl std::fmt::Debug for OutboundUniStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("OutboundUniStream").finish_non_exhaustive()
    }
}

impl OutboundUniStream {
    pub(crate) fn new(stream: BoxedSendStream) -> Self {
        Self(stream)
    }
}

fn read_buffered(
    buffer: &mut Vec<u8>,
    offset: &mut usize,
    transport: &mut (impl AsyncRead + Unpin),
    cx: &mut Context<'_>,
    buf: &mut [u8],
) -> Poll<io::Result<usize>> {
    let remaining = buffer.len() - *offset;
    if remaining == 0 {
        return Pin::new(transport).poll_read(cx, buf);
    }

    let n = remaining.min(buf.len());
    buf[..n].copy_from_slice(&buffer[*offset..*offset + n]);
    *offset += n;

    if *offset == buffer.len() {
        *buffer = Vec::new();
        *offset = 0;
    }

    Poll::Ready(Ok(n))
}
