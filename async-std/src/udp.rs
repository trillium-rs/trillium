use async_io::Async;
#[cfg(unix)]
use std::os::unix::io::{AsFd, BorrowedFd};
#[cfg(windows)]
use std::os::windows::io::{AsSocket, BorrowedSocket};
use std::{
    io,
    net::{SocketAddr, UdpSocket},
    task::{Context, Poll, ready},
};
use trillium_server_common::UdpTransport;

/// Async-io-backed UDP socket for use with QUIC transports.
#[derive(Debug)]
pub struct AsyncStdUdpSocket(Async<UdpSocket>);

impl UdpTransport for AsyncStdUdpSocket {
    fn from_std(socket: UdpSocket) -> io::Result<Self> {
        Async::new(socket).map(Self)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.0.get_ref().local_addr()
    }

    fn poll_recv_io<R>(
        &self,
        cx: &mut Context<'_>,
        mut recv: impl FnMut(&Self) -> io::Result<R>,
    ) -> Poll<io::Result<R>> {
        loop {
            ready!(self.0.poll_readable(cx))?;
            match recv(self) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                result => return Poll::Ready(result),
            }
        }
    }

    fn poll_writable(&self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.0.poll_writable(cx)
    }

    fn try_send_io<R>(&self, send: impl FnOnce(&Self) -> io::Result<R>) -> io::Result<R> {
        send(self)
    }
}

#[cfg(unix)]
impl AsFd for AsyncStdUdpSocket {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

#[cfg(windows)]
impl AsSocket for AsyncStdUdpSocket {
    fn as_socket(&self) -> BorrowedSocket<'_> {
        self.0.as_socket()
    }
}
