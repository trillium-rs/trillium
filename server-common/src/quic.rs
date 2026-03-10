use crate::{Server, Transport};
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    fmt::Debug,
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use trillium::Info;
use trillium_http::transport::BoxedTransport;

/// Abstraction over a single QUIC connection.
///
/// QUIC library adapters (e.g. trillium-quinn) implement this trait. The generic HTTP/3 connection
/// handler in server-common consumes it to manage streams without knowing about the underlying QUIC
/// implementation.
///
/// Implementations should be cheaply cloneable (typically wrapping an `Arc`-based connection
/// handle) since the connection handler clones this into spawned tasks.
pub trait QuicConnectionTrait: Clone + Send + Sync + 'static {
    /// A bidirectional stream, used for HTTP/3 request/response pairs.
    type BidiStream: Transport;

    /// A unidirectional receive stream from the peer (control, QPACK, etc.).
    type RecvStream: AsyncRead + Unpin + Send + Sync + 'static;

    /// A unidirectional send stream to the peer (control, QPACK, etc.).
    type SendStream: AsyncWrite + Unpin + Send + Sync + 'static;

    /// Accept the next bidirectional stream opened by the peer.
    ///
    /// Returns the QUIC stream ID and a combined read/write transport. The stream ID is used
    /// for GOAWAY tracking and should be passed to [`H3Connection::process_inbound_bidi`].
    fn accept_bidi(&self) -> impl Future<Output = io::Result<(u64, Self::BidiStream)>> + Send;

    /// Accept the next unidirectional stream opened by the peer.
    ///
    /// Returns the QUIC stream ID and a receive-only stream.
    fn accept_uni(&self) -> impl Future<Output = io::Result<(u64, Self::RecvStream)>> + Send;

    /// Open a new unidirectional stream to the peer.
    ///
    /// Returns the QUIC stream ID and a send-only stream.
    fn open_uni(&self) -> impl Future<Output = io::Result<(u64, Self::SendStream)>> + Send;

    /// Open a new bidirectional stream to the peer.
    ///
    /// Returns the QUIC stream ID and a combined read/write transport.
    fn open_bidi(&self) -> impl Future<Output = io::Result<(u64, Self::BidiStream)>> + Send;

    /// The peer's address.
    fn remote_address(&self) -> SocketAddr;

    /// Close the entire QUIC connection with an error code and reason.
    fn close(&self, error_code: u64, reason: &[u8]);

    /// Stop a unidirectional receive stream, signaling an error code to the peer.
    ///
    /// Sends STOP_SENDING. Takes ownership of the stream since it cannot be read after stopping.
    fn stop_uni(&self, stream: Self::RecvStream, error_code: u64);

    /// Stop a bidirectional stream, signaling an error code to the peer.
    ///
    /// Sends STOP_SENDING on the receive side and RESET_STREAM on the send side.
    /// Takes ownership of the stream since it cannot be used after stopping.
    fn stop_bidi(&self, stream: Self::BidiStream, error_code: u64);

    /// Send an unreliable datagram over the QUIC connection.
    ///
    /// Datagrams are atomic and unordered. The data must fit in a single QUIC packet
    /// (typically ~1200 bytes). Returns an error if datagrams are not supported by the
    /// peer or the data is too large.
    fn send_datagram(&self, data: &[u8]) -> io::Result<()>;

    /// Receive the next unreliable datagram from the peer, passing the raw bytes to `callback`.
    fn recv_datagram<F: FnOnce(&[u8]) + Send>(
        &self,
        callback: F,
    ) -> impl Future<Output = io::Result<()>> + Send;

    /// The maximum datagram payload size the peer will accept, if datagrams are supported.
    ///
    /// Returns `None` if the peer does not support datagrams.
    fn max_datagram_size(&self) -> Option<usize>;
}

/// Configuration for a QUIC endpoint, provided by the user at server setup time.
///
/// QUIC library adapters implement this (e.g. `trillium_quinn::QuicConfig`). The `()`
/// implementation produces no binding (HTTP/3 disabled).
///
/// The generic flow is:
/// 1. User provides a `QuicConfig` via [`Config::with_quic`](crate::Config)
/// 2. During server startup, `bind` is called with the TCP listener's address and runtime
/// 3. The resulting [`QuicBinding`] is stored on `RunningConfig` and drives the H3 accept loop
pub trait QuicConfig<S: Server>: Send + 'static {
    /// The bound endpoint type produced by [`bind`](QuicConfig::bind).
    type Binding: QuicBinding;

    /// Bind a QUIC endpoint to the given address.
    ///
    /// The runtime is provided so that QUIC library adapters can bridge
    /// to the active async runtime for timers, spawning, and UDP I/O.
    ///
    /// Returns `None` if QUIC is not configured (the `()` case), `Some(Ok(binding))` on success,
    /// or `Some(Err(..))` if binding fails.
    fn bind(
        self,
        addr: SocketAddr,
        runtime: S::Runtime,
        info: &mut Info,
    ) -> Option<io::Result<Self::Binding>>;
}

impl<S: Server> QuicConfig<S> for () {
    type Binding = ();

    fn bind(self, _: SocketAddr, _: S::Runtime, _: &mut Info) -> Option<io::Result<()>> {
        None
    }
}

/// A bound QUIC endpoint that accepts connections.
///
/// Analogous to [`Server`](crate::Server) for TCP. QUIC library adapters implement this to provide
/// the connection accept loop. The `()` implementation accepts no connections (HTTP/3 disabled).
pub trait QuicBinding: Send + Sync + 'static {
    /// The connection type yielded by this endpoint.
    type Connection: QuicConnectionTrait;

    /// Accept the next QUIC connection, or return `None` if the endpoint is done.
    fn accept(&self) -> impl Future<Output = Option<Self::Connection>> + Send;
}

/// Uninhabited type used by the `()` [`QuicBinding`] implementation.
///
/// Since `()` never produces connections, this type is never constructed and its trait
/// implementations are never exercised.
#[derive(Debug, Clone, Copy)]
pub enum NoQuic {}

impl QuicConnectionTrait for NoQuic {
    type BidiStream = NoQuic;
    type RecvStream = NoQuic;
    type SendStream = NoQuic;

    async fn accept_bidi(&self) -> io::Result<(u64, Self::BidiStream)> {
        match *self {}
    }

    async fn accept_uni(&self) -> io::Result<(u64, Self::RecvStream)> {
        match *self {}
    }

    async fn open_uni(&self) -> io::Result<(u64, Self::SendStream)> {
        match *self {}
    }

    async fn open_bidi(&self) -> io::Result<(u64, Self::BidiStream)> {
        match *self {}
    }

    fn remote_address(&self) -> SocketAddr {
        match *self {}
    }

    fn close(&self, _: u64, _: &[u8]) {
        match *self {}
    }

    fn stop_uni(&self, stream: Self::RecvStream, _: u64) {
        match stream {}
    }

    fn stop_bidi(&self, stream: Self::BidiStream, _: u64) {
        match stream {}
    }

    fn send_datagram(&self, _: &[u8]) -> io::Result<()> {
        match *self {}
    }

    async fn recv_datagram<F: FnOnce(&[u8]) + Send>(&self, _: F) -> io::Result<()> {
        match *self {}
    }

    fn max_datagram_size(&self) -> Option<usize> {
        match *self {}
    }
}

impl Transport for NoQuic {}

impl AsyncRead for NoQuic {
    fn poll_read(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        _: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match *self.get_mut() {}
    }
}

impl AsyncWrite for NoQuic {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, _: &[u8]) -> Poll<io::Result<usize>> {
        match *self.get_mut() {}
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        match *self.get_mut() {}
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        match *self.get_mut() {}
    }
}

impl QuicBinding for () {
    type Connection = NoQuic;

    async fn accept(&self) -> Option<NoQuic> {
        None
    }
}

// -- Type-erased QuicConnection --

type BoxedFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
type BoxedRecvStream = Box<dyn AsyncRead + Unpin + Send + Sync>;
type BoxedSendStream = Box<dyn AsyncWrite + Unpin + Send + Sync>;

trait ObjectSafeQuicConnection: Send + Sync {
    fn accept_bidi(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedTransport)>>;
    fn accept_uni(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedRecvStream)>>;
    fn open_uni(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedSendStream)>>;
    fn open_bidi(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedTransport)>>;
    fn remote_address(&self) -> SocketAddr;
    fn close(&self, error_code: u64, reason: &[u8]);
    fn send_datagram(&self, data: &[u8]) -> io::Result<()>;
    fn recv_datagram<'a>(
        &'a self,
        callback: Box<dyn FnOnce(&[u8]) + Send + 'a>,
    ) -> BoxedFuture<'a, io::Result<()>>;
    fn max_datagram_size(&self) -> Option<usize>;
}

impl<T: QuicConnectionTrait> ObjectSafeQuicConnection for T {
    fn accept_bidi(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedTransport)>> {
        Box::pin(async {
            let (id, stream) = QuicConnectionTrait::accept_bidi(self).await?;
            Ok((id, BoxedTransport::new(stream)))
        })
    }

    fn accept_uni(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedRecvStream)>> {
        Box::pin(async {
            let (id, stream) = QuicConnectionTrait::accept_uni(self).await?;
            Ok((id, Box::new(stream) as BoxedRecvStream))
        })
    }

    fn open_uni(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedSendStream)>> {
        Box::pin(async {
            let (id, stream) = QuicConnectionTrait::open_uni(self).await?;
            Ok((id, Box::new(stream) as BoxedSendStream))
        })
    }

    fn open_bidi(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedTransport)>> {
        Box::pin(async {
            let (id, stream) = QuicConnectionTrait::open_bidi(self).await?;
            Ok((id, BoxedTransport::new(stream)))
        })
    }

    fn remote_address(&self) -> SocketAddr {
        QuicConnectionTrait::remote_address(self)
    }

    fn close(&self, error_code: u64, reason: &[u8]) {
        QuicConnectionTrait::close(self, error_code, reason)
    }

    fn send_datagram(&self, data: &[u8]) -> io::Result<()> {
        QuicConnectionTrait::send_datagram(self, data)
    }

    fn recv_datagram<'a>(
        &'a self,
        callback: Box<dyn FnOnce(&[u8]) + Send + 'a>,
    ) -> BoxedFuture<'a, io::Result<()>> {
        Box::pin(QuicConnectionTrait::recv_datagram(self, callback))
    }

    fn max_datagram_size(&self) -> Option<usize> {
        QuicConnectionTrait::max_datagram_size(self)
    }
}

/// A type-erased QUIC connection handle, equivalent to `Arc<dyn QuicConnectionTrait>`.
/// Cheaply cloneable.
///
/// Handlers retrieve this from conn state to access QUIC features (streams, datagrams)
/// without depending on the concrete QUIC implementation type.
#[derive(Clone)]
pub struct QuicConnection(Arc<dyn ObjectSafeQuicConnection>);

impl Debug for QuicConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicConnection")
            .field("peer", &self.remote_address())
            .finish_non_exhaustive()
    }
}

impl<T: QuicConnectionTrait> From<T> for QuicConnection {
    fn from(connection: T) -> Self {
        Self(Arc::new(connection))
    }
}

impl QuicConnection {
    /// Accept the next bidirectional stream opened by the peer.
    pub async fn accept_bidi(&self) -> io::Result<(u64, BoxedTransport)> {
        self.0.accept_bidi().await
    }

    /// Accept the next unidirectional stream opened by the peer.
    pub async fn accept_uni(&self) -> io::Result<(u64, BoxedRecvStream)> {
        self.0.accept_uni().await
    }

    /// Open a new unidirectional stream to the peer.
    pub async fn open_uni(&self) -> io::Result<(u64, BoxedSendStream)> {
        self.0.open_uni().await
    }

    /// Open a new bidirectional stream to the peer.
    pub async fn open_bidi(&self) -> io::Result<(u64, BoxedTransport)> {
        self.0.open_bidi().await
    }

    /// The peer's address.
    pub fn remote_address(&self) -> SocketAddr {
        self.0.remote_address()
    }

    /// Close the entire QUIC connection with an error code and reason.
    pub fn close(&self, error_code: u64, reason: &[u8]) {
        self.0.close(error_code, reason)
    }

    /// Send an unreliable datagram over the QUIC connection.
    pub fn send_datagram(&self, data: &[u8]) -> io::Result<()> {
        self.0.send_datagram(data)
    }

    /// Receive the next unreliable datagram from the peer, passing the raw bytes to `callback`.
    pub async fn recv_datagram<'a, F: FnOnce(&[u8]) + Send + 'a>(
        &'a self,
        callback: F,
    ) -> io::Result<()> {
        self.0.recv_datagram(Box::new(callback)).await
    }

    /// The maximum datagram payload size the peer will accept, if datagrams are supported.
    pub fn max_datagram_size(&self) -> Option<usize> {
        self.0.max_datagram_size()
    }
}
