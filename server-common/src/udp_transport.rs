use std::{
    fmt::Debug,
    io::{self, ErrorKind},
    net::{SocketAddr, UdpSocket},
    task::{Context, Poll},
};

/// Async UDP socket abstraction for QUIC transport.
///
/// Runtime adapters implement this for their platform's async UDP type.
/// QUIC library adapters (e.g. trillium-quinn) consume this to bridge
/// to their own socket traits.
///
/// The `poll_recv_io` and `try_send_io` methods pass `&Self` to the
/// caller's closure, allowing the caller to access platform-specific
/// traits (e.g. `AsFd` on unix, `AsSocket` on windows) without those
/// traits appearing in this trait's definition.
///
/// Runtimes that do not support UDP can use `()` as their
/// `UdpTransport` type — it returns errors from all operations.
pub trait UdpTransport: Send + Sync + Debug + Sized + 'static {
    /// Wrap a bound, non-blocking std UDP socket into this async type.
    fn from_std(socket: UdpSocket) -> io::Result<Self>;

    /// The local address this socket is bound to.
    fn local_addr(&self) -> io::Result<SocketAddr>;

    /// Poll for read readiness, then attempt a receive operation.
    ///
    /// When the socket is readable, calls `recv` with `&self`. If
    /// `recv` returns [`ErrorKind::WouldBlock`], the implementation
    /// clears readiness and re-polls on the next call.
    fn poll_recv_io<R>(
        &self,
        cx: &mut Context<'_>,
        recv: impl FnMut(&Self) -> io::Result<R>,
    ) -> Poll<io::Result<R>>;

    /// Poll for write readiness without attempting any I/O.
    ///
    /// Used by QUIC implementations that separate readiness polling
    /// from the send attempt (e.g. quinn's multi-sender pattern).
    fn poll_writable(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>>;

    /// Attempt a send operation, managing readiness state.
    ///
    /// Calls `send` with `&self`. On [`ErrorKind::WouldBlock`], the
    /// implementation ensures the next [`poll_writable`](UdpTransport::poll_writable)
    /// call returns [`Poll::Pending`].
    fn try_send_io<R>(&self, send: impl FnOnce(&Self) -> io::Result<R>) -> io::Result<R>;

    /// Maximum number of datagrams to send in a single syscall (GSO).
    fn max_transmit_segments(&self) -> usize {
        1
    }

    /// Maximum number of datagrams to receive in a single syscall (GRO).
    fn max_receive_segments(&self) -> usize {
        1
    }

    /// Whether outbound datagrams may be fragmented by the network layer.
    fn may_fragment(&self) -> bool {
        true
    }
}

fn unsupported() -> io::Error {
    io::Error::new(ErrorKind::Unsupported, "UDP not supported by this runtime")
}

impl UdpTransport for () {
    fn from_std(_: UdpSocket) -> io::Result<Self> {
        Err(unsupported())
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Err(unsupported())
    }

    fn poll_recv_io<R>(
        &self,
        _: &mut Context<'_>,
        _: impl FnMut(&Self) -> io::Result<R>,
    ) -> Poll<io::Result<R>> {
        Poll::Ready(Err(unsupported()))
    }

    fn poll_writable(&self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Err(unsupported()))
    }

    fn try_send_io<R>(&self, _: impl FnOnce(&Self) -> io::Result<R>) -> io::Result<R> {
        Err(unsupported())
    }
}
