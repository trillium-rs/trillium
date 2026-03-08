#[cfg(unix)]
use std::os::unix::io::{AsFd, BorrowedFd};
#[cfg(windows)]
use std::os::windows::io::{AsSocket, BorrowedSocket};
use std::{
    io,
    net::{SocketAddr, UdpSocket},
    task::{Context, Poll, ready},
};
use tokio::io::Interest;
use trillium_server_common::UdpTransport;

/// Tokio-backed async UDP socket for use with QUIC transports.
#[derive(Debug)]
pub struct TokioUdpSocket(tokio::net::UdpSocket);

impl UdpTransport for TokioUdpSocket {
    fn from_std(socket: UdpSocket) -> io::Result<Self> {
        socket.set_nonblocking(true)?;
        tokio::net::UdpSocket::from_std(socket).map(Self)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.local_addr()
    }

    fn poll_recv_io<R>(
        &self,
        cx: &mut Context<'_>,
        mut recv: impl FnMut(&Self) -> io::Result<R>,
    ) -> Poll<io::Result<R>> {
        loop {
            ready!(self.0.poll_recv_ready(cx))?;
            match self.0.try_io(Interest::READABLE, || recv(self)) {
                Ok(result) => return Poll::Ready(Ok(result)),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_writable(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.poll_send_ready(cx)
    }

    fn try_send_io<R>(&self, send: impl FnOnce(&Self) -> io::Result<R>) -> io::Result<R> {
        self.0.try_io(Interest::WRITABLE, || send(self))
    }
}

#[cfg(unix)]
impl AsFd for TokioUdpSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

#[cfg(windows)]
impl AsSocket for TokioUdpSocket {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        self.0.as_socket()
    }
}
