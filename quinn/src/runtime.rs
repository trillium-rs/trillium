use quinn::udp;
#[cfg(unix)]
use std::os::unix::io::AsFd;
#[cfg(windows)]
use std::os::windows::io::AsSocket;
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    io,
    marker::PhantomData,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};
use trillium_server_common::{RuntimeTrait, UdpTransport};

/// Platform-conditional bound: UdpTransport + raw socket access.
///
/// quinn-udp's `UdpSocketState` needs a raw fd/socket to perform
/// platform-optimized send/recv syscalls. This bound is satisfied by
/// all trillium runtime adapters' UDP socket types.
#[cfg(unix)]
pub(crate) trait SocketTransport: UdpTransport + AsFd {}
#[cfg(unix)]
impl<T: UdpTransport + AsFd> SocketTransport for T {}

#[cfg(windows)]
pub(crate) trait SocketTransport: UdpTransport + AsSocket {}
#[cfg(windows)]
impl<T: UdpTransport + AsSocket> SocketTransport for T {}

/// Bridges trillium's [`RuntimeTrait`] + [`UdpTransport`] to quinn's
/// [`Runtime`](quinn::Runtime) trait, making quinn runtime-agnostic.
pub(crate) struct TrilliumRuntime<R: RuntimeTrait + Unpin, U: SocketTransport> {
    runtime: R,
    _udp: PhantomData<U>,
}

impl<R: RuntimeTrait + Unpin, U: SocketTransport> TrilliumRuntime<R, U> {
    pub(crate) fn new(runtime: R) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            _udp: PhantomData,
        })
    }
}

impl<R: RuntimeTrait + Unpin, U: SocketTransport> Debug for TrilliumRuntime<R, U> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrilliumRuntime").finish_non_exhaustive()
    }
}

impl<R: RuntimeTrait + Unpin, U: SocketTransport> quinn::Runtime for TrilliumRuntime<R, U> {
    fn new_timer(&self, i: Instant) -> Pin<Box<dyn quinn::AsyncTimer>> {
        Box::pin(Timer::new(self.runtime.clone(), i))
    }

    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + Send>>) {
        self.runtime.spawn(future);
    }

    fn wrap_udp_socket(
        &self,
        sock: std::net::UdpSocket,
    ) -> io::Result<Arc<dyn quinn::AsyncUdpSocket>> {
        let transport = U::from_std(sock)?;
        let inner = udp::UdpSocketState::new(udp::UdpSockRef::from(&transport))?;
        Ok(Arc::new(UdpSocket { inner, transport }))
    }
}

// --- Resettable timer ---

/// Resettable timer backed by trillium's one-shot `delay`.
///
/// On each `reset`, the current delay future is replaced with a new
/// one targeting the updated deadline.
struct Timer<R> {
    runtime: R,
    deadline: Instant,
    delay: Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl<R: RuntimeTrait> Timer<R> {
    fn new(runtime: R, deadline: Instant) -> Self {
        let delay = Self::make_delay(&runtime, deadline);
        Self {
            runtime,
            deadline,
            delay,
        }
    }

    fn make_delay(runtime: &R, deadline: Instant) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let duration = deadline.saturating_duration_since(Instant::now());
        let runtime = runtime.clone();
        Box::pin(async move { runtime.delay(duration).await })
    }
}

impl<R: RuntimeTrait> Debug for Timer<R> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Timer")
            .field("deadline", &self.deadline)
            .finish()
    }
}

impl<R: RuntimeTrait + Unpin> quinn::AsyncTimer for Timer<R> {
    fn reset(self: Pin<&mut Self>, i: Instant) {
        let this = self.get_mut();
        this.deadline = i;
        this.delay = Self::make_delay(&this.runtime, i);
    }

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.get_mut().delay.as_mut().poll(cx)
    }
}

// --- AsyncUdpSocket backed by UdpTransport ---

struct UdpSocket<U> {
    inner: udp::UdpSocketState,
    transport: U,
}

impl<U: SocketTransport> Debug for UdpSocket<U> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UdpSocket").finish_non_exhaustive()
    }
}

impl<U: SocketTransport> quinn::AsyncUdpSocket for UdpSocket<U> {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn quinn::UdpPoller>> {
        Box::pin(UdpPoller { socket: self })
    }

    fn try_send(&self, transmit: &udp::Transmit<'_>) -> io::Result<()> {
        self.transport
            .try_send_io(|t| self.inner.send(udp::UdpSockRef::from(t), transmit))
    }

    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [io::IoSliceMut<'_>],
        meta: &mut [udp::RecvMeta],
    ) -> Poll<io::Result<usize>> {
        self.transport.poll_recv_io(cx, |t| {
            self.inner.recv(udp::UdpSockRef::from(t), bufs, meta)
        })
    }

    fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.transport.local_addr()
    }

    fn max_transmit_segments(&self) -> usize {
        self.inner.max_gso_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.inner.gro_segments()
    }

    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}

// --- UdpPoller ---

struct UdpPoller<U> {
    socket: Arc<UdpSocket<U>>,
}

impl<U: SocketTransport> Debug for UdpPoller<U> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("UdpPoller").finish_non_exhaustive()
    }
}

impl<U: SocketTransport> quinn::UdpPoller for UdpPoller<U> {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.socket.transport.poll_writable(cx)
    }
}
