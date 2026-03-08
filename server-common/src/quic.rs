use crate::{Server, Transport};
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    future::Future,
    io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use trillium::Info;

/// Abstraction over a single QUIC connection.
///
/// QUIC library adapters (e.g. trillium-quinn) implement this trait. The generic HTTP/3 connection
/// handler in server-common consumes it to manage streams without knowing about the underlying QUIC
/// implementation.
///
/// Implementations should be cheaply cloneable (typically wrapping an `Arc`-based connection
/// handle) since the connection handler clones this into spawned tasks.
pub trait QuicConnection: Clone + Send + Sync + 'static {
    /// A bidirectional stream, used for HTTP/3 request/response pairs.
    type BidiStream: Transport;

    /// A unidirectional receive stream from the peer (control, QPACK, etc.).
    type RecvStream: AsyncRead + Unpin + Send + Sync + 'static;

    /// A unidirectional send stream to the peer (control, QPACK, etc.).
    type SendStream: AsyncWrite + Unpin + Send + Sync + 'static;

    /// Accept the next bidirectional stream opened by the peer.
    ///
    /// Returns the QUIC stream ID and a combined read/write transport.
    /// The stream ID is passed to [`H3Connection::run_request`] for GOAWAY tracking.
    fn accept_bi(&self) -> impl Future<Output = io::Result<(u64, Self::BidiStream)>> + Send;

    /// Accept the next unidirectional stream opened by the peer.
    fn accept_uni(&self) -> impl Future<Output = io::Result<Self::RecvStream>> + Send;

    /// Open a new unidirectional stream to the peer.
    fn open_uni(&self) -> impl Future<Output = io::Result<Self::SendStream>> + Send;

    /// The peer's address.
    fn remote_address(&self) -> SocketAddr;

    /// Close the entire QUIC connection with an error code and reason.
    fn close(&self, error_code: u64, reason: &[u8]);

    /// Stop a receive stream, signaling an error code to the peer.
    ///
    /// Takes ownership of the stream since it cannot be read after stopping.
    fn stop_stream(&self, stream: Self::RecvStream, error_code: u64);

    /// Send an unreliable datagram over the QUIC connection.
    ///
    /// Datagrams are atomic and unordered. The data must fit in a single QUIC packet
    /// (typically ~1200 bytes). Returns an error if datagrams are not supported by the
    /// peer or the data is too large.
    fn send_datagram(&self, data: &[u8]) -> io::Result<()>;

    /// Receive the next unreliable datagram from the peer.
    ///
    /// The datagram payload is appended to `buf`. Returns the number of bytes received.
    fn recv_datagram(
        &self,
        buf: &mut (impl Extend<u8> + Send),
    ) -> impl Future<Output = io::Result<usize>> + Send;

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
    type Connection: QuicConnection;

    /// Accept the next QUIC connection, or return `None` if the endpoint is done.
    fn accept(&self) -> impl Future<Output = Option<Self::Connection>> + Send;
}

/// Uninhabited type used by the `()` [`QuicBinding`] implementation.
///
/// Since `()` never produces connections, this type is never constructed and its trait
/// implementations are never exercised.
#[derive(Debug, Clone, Copy)]
pub enum NoQuic {}

impl QuicConnection for NoQuic {
    type BidiStream = NoQuic;
    type RecvStream = NoQuic;
    type SendStream = NoQuic;

    async fn accept_bi(&self) -> io::Result<(u64, Self::BidiStream)> {
        match *self {}
    }

    async fn accept_uni(&self) -> io::Result<Self::RecvStream> {
        match *self {}
    }

    async fn open_uni(&self) -> io::Result<Self::SendStream> {
        match *self {}
    }

    fn remote_address(&self) -> SocketAddr {
        match *self {}
    }

    fn close(&self, _: u64, _: &[u8]) {
        match *self {}
    }

    fn stop_stream(&self, stream: Self::RecvStream, _: u64) {
        match stream {}
    }

    fn send_datagram(&self, _: &[u8]) -> io::Result<()> {
        match *self {}
    }

    async fn recv_datagram(&self, _: &mut (impl Extend<u8> + Send)) -> io::Result<usize> {
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
