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

/// Abstraction over the inbound half of a QUIC stream (both bidi and inbound uni)
pub trait QuicTransportReceive: AsyncRead {
    /// Stop a receive stream, signaling an error code to the peer.
    fn stop(&mut self, code: u64);
}

/// Abstraction over the outbound half of a QUIC stream (both bidi and outbound uni)
pub trait QuicTransportSend: AsyncWrite {
    /// Close the send stream immediately with the provided error code.
    fn reset(&mut self, code: u64);
}

/// Abstraction over a QUIC bidirectional stream
pub trait QuicTransportBidi: QuicTransportReceive + QuicTransportSend + Transport {}

/// Abstraction over a single QUIC connection.
///
/// QUIC library adapters (e.g. trillium-quinn) implement this trait. The generic HTTP/3 connection
/// handler in server-common consumes it to manage streams without knowing about the underlying QUIC
/// implementation.
///
/// Implementations should be cheaply cloneable (typically wrapping an `Arc`-based connection
/// handle) since the connection handler clones this into spawned tasks.
pub trait QuicConnectionTrait: Clone + Send + Sync + 'static {
    /// A bidirectional stream
    type BidiStream: QuicTransportBidi + Unpin + Send + Sync + 'static;

    /// A unidirectional receive stream from the peer
    type RecvStream: QuicTransportReceive + Unpin + Send + Sync + 'static;

    /// A unidirectional send stream to the peer
    type SendStream: QuicTransportSend + Unpin + Send + Sync + 'static;

    /// Accept the next bidirectional stream opened by the peer.
    ///
    /// Returns the QUIC stream ID and a combined read/write transport.
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
/// 3. The resulting [`QuicEndpoint`] is stored on `RunningConfig` and drives the H3 accept loop
pub trait QuicConfig<S: Server>: Send + 'static {
    /// The bound endpoint type produced by [`bind`](QuicConfig::bind).
    type Endpoint: QuicEndpoint;

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
    ) -> Option<io::Result<Self::Endpoint>>;
}

impl<S: Server> QuicConfig<S> for () {
    type Endpoint = ();

    fn bind(self, _: SocketAddr, _: S::Runtime, _: &mut Info) -> Option<io::Result<()>> {
        None
    }
}

/// A bound QUIC endpoint that accepts and initiates connections.
///
/// Analogous to [`Server`](crate::Server) for TCP. QUIC library adapters implement this to provide
/// the connection accept loop (server) and outbound connections (client).
///
/// The `()` implementation is a no-op (HTTP/3 disabled). Server-only implementations may return
/// an error from [`connect`](QuicEndpoint::connect); client-only implementations may return
/// `None` from [`accept`](QuicEndpoint::accept).
pub trait QuicEndpoint: Send + Sync + 'static {
    /// The connection type yielded by this endpoint.
    type Connection: QuicConnectionTrait;

    /// Accept the next inbound QUIC connection, or return `None` if the endpoint is done.
    fn accept(&self) -> impl Future<Output = Option<Self::Connection>> + Send;

    /// Initiate a QUIC connection to the given address.
    ///
    /// `server_name` is the SNI hostname used for TLS verification.
    fn connect(
        &self,
        addr: SocketAddr,
        server_name: &str,
    ) -> impl Future<Output = io::Result<Self::Connection>> + Send;
}

/// Uninhabited type used by the `()` [`QuicBinding`] implementation.
///
/// Since `()` never produces connections, this type is never constructed and its trait
/// implementations are never exercised.
#[derive(Debug, Clone, Copy)]
pub enum NoQuic {}

impl QuicTransportSend for NoQuic {
    fn reset(&mut self, _code: u64) {
        match *self {}
    }
}

impl QuicTransportReceive for NoQuic {
    fn stop(&mut self, _code: u64) {
        match *self {}
    }
}

impl QuicTransportBidi for NoQuic {}

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

impl QuicEndpoint for () {
    type Connection = NoQuic;

    async fn accept(&self) -> Option<NoQuic> {
        None
    }

    async fn connect(&self, _: SocketAddr, _: &str) -> io::Result<NoQuic> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "QUIC not configured",
        ))
    }
}

// -- Type-erased QuicConnection --

type BoxedFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub(crate) type BoxedRecvStream = Box<dyn QuicTransportReceive + Unpin + Send + Sync>;
pub(crate) type BoxedSendStream = Box<dyn QuicTransportSend + Unpin + Send + Sync>;
pub(crate) type BoxedBidiStream = Box<dyn QuicTransportBidi + Unpin + Send + Sync>;

type ReceiveDatagramCallback<'a> = Box<dyn FnOnce(&[u8]) + Send + 'a>;

trait ObjectSafeQuicConnection: Send + Sync {
    fn accept_bidi(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedBidiStream)>>;
    fn accept_uni(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedRecvStream)>>;
    fn open_uni(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedSendStream)>>;
    fn open_bidi(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedBidiStream)>>;
    fn remote_address(&self) -> SocketAddr;
    fn close(&self, error_code: u64, reason: &[u8]);
    fn send_datagram(&self, data: &[u8]) -> io::Result<()>;
    fn recv_datagram<'a>(
        &'a self,
        callback: ReceiveDatagramCallback<'a>,
    ) -> BoxedFuture<'a, io::Result<()>>;
    fn max_datagram_size(&self) -> Option<usize>;
}

impl<T: QuicConnectionTrait> ObjectSafeQuicConnection for T {
    fn accept_bidi(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedBidiStream)>> {
        Box::pin(async {
            let (id, stream) = QuicConnectionTrait::accept_bidi(self).await?;
            Ok((id, Box::new(stream) as BoxedBidiStream))
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

    fn open_bidi(&self) -> BoxedFuture<'_, io::Result<(u64, BoxedBidiStream)>> {
        Box::pin(async {
            let (id, stream) = QuicConnectionTrait::open_bidi(self).await?;
            Ok((id, Box::new(stream) as BoxedBidiStream))
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
    pub async fn accept_bidi(&self) -> io::Result<(u64, BoxedBidiStream)> {
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
    pub async fn open_bidi(&self) -> io::Result<(u64, BoxedBidiStream)> {
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

// -- Type-erased QuicEndpoint --

trait ObjectSafeQuicEndpoint: Send + Sync {
    fn accept(&self) -> BoxedFuture<'_, Option<QuicConnection>>;
    fn connect<'a>(
        &'a self,
        addr: SocketAddr,
        server_name: &'a str,
    ) -> BoxedFuture<'a, io::Result<QuicConnection>>;
}

impl<T: QuicEndpoint> ObjectSafeQuicEndpoint for T {
    fn accept(&self) -> BoxedFuture<'_, Option<QuicConnection>> {
        Box::pin(async { QuicEndpoint::accept(self).await.map(QuicConnection::from) })
    }

    fn connect<'a>(
        &'a self,
        addr: SocketAddr,
        server_name: &'a str,
    ) -> BoxedFuture<'a, io::Result<QuicConnection>> {
        Box::pin(async move {
            QuicEndpoint::connect(self, addr, server_name)
                .await
                .map(QuicConnection::from)
        })
    }
}

/// A type-erased QUIC endpoint, equivalent to `Arc<dyn QuicEndpoint>`.
/// Cheaply cloneable.
#[derive(Clone)]
pub struct ArcedQuicEndpoint(Arc<dyn ObjectSafeQuicEndpoint>);

impl Debug for ArcedQuicEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ArcedQuicEndpoint").finish()
    }
}

impl<T: QuicEndpoint> From<T> for ArcedQuicEndpoint {
    fn from(endpoint: T) -> Self {
        Self(Arc::new(endpoint))
    }
}

impl ArcedQuicEndpoint {
    /// Accept the next inbound QUIC connection.
    pub async fn accept(&self) -> Option<QuicConnection> {
        self.0.accept().await
    }

    /// Initiate a QUIC connection to the given address.
    pub async fn connect(&self, addr: SocketAddr, server_name: &str) -> io::Result<QuicConnection> {
        self.0.connect(addr, server_name).await
    }
}
