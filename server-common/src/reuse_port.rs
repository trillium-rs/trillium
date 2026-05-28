use socket2::{Domain, Socket, Type};
use std::{
    io,
    net::{SocketAddr, TcpListener},
};

/// The listen backlog applied to reuseport listeners. Larger than the OS default to absorb
/// connection bursts that would otherwise be dropped before any accept loop can dequeue them.
const LISTEN_BACKLOG: i32 = 1024;

/// Bind a TCP listener with `SO_REUSEPORT` and `SO_REUSEADDR` set.
///
/// Multiple listeners bound to the same concrete address form a kernel reuseport group, across
/// which the kernel load-balances inbound connections — letting a server fan accepts out across
/// per-core listeners with no userspace dispatch. The returned listener is non-blocking, ready to
/// be adopted by a runtime (e.g. `tokio::net::TcpListener::from_std`).
///
/// To claim an ephemeral port for a group, bind once with a port of `0`, read the assigned port
/// from [`TcpListener::local_addr`], and bind the remaining members to that resolved address.
///
/// Reuseport's kernel fan-out is a Linux feature; other Unixes accept the options but distribute
/// connections differently or not at all. This is unavailable on Windows, which has no
/// `SO_REUSEPORT`.
pub fn bind_reuse_port(addr: SocketAddr) -> io::Result<TcpListener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, None)?;
    socket.set_reuse_port(true)?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(LISTEN_BACKLOG)?;
    Ok(socket.into())
}
