use std::{
    fmt::Display,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6, ToSocketAddrs},
};

/// Conversion into a single TCP bind address.
///
/// Implemented for the address-shaped inputs — a bare port (binds `0.0.0.0:port`), a [`SocketAddr`]
/// or its v4/v6 forms, and `(ip, port)` tuples — all of which are infallible, and for the string
/// and `(host, port)` forms, which are resolved through the system resolver and so can fail or
/// yield no address. A resolving form binds the *first* address it resolves to; pass a
/// [`SocketAddr`] when you need to pin exactly which one.
pub trait IntoListenAddr {
    /// Resolve to the concrete socket address to bind, surfacing a resolution failure as an
    /// [`io::Error`].
    fn into_listen_addr(self) -> io::Result<SocketAddr>;
}

/// Resolve through [`ToSocketAddrs`] and take the first address, reporting an empty resolution as
/// an error that names what failed to resolve.
fn resolve_first(addr: impl ToSocketAddrs, label: impl Display) -> io::Result<SocketAddr> {
    addr.to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::other(format!("`{label}` did not resolve to a bind address")))
}

impl IntoListenAddr for SocketAddr {
    fn into_listen_addr(self) -> io::Result<SocketAddr> {
        Ok(self)
    }
}

impl IntoListenAddr for SocketAddrV4 {
    fn into_listen_addr(self) -> io::Result<SocketAddr> {
        Ok(self.into())
    }
}

impl IntoListenAddr for SocketAddrV6 {
    fn into_listen_addr(self) -> io::Result<SocketAddr> {
        Ok(self.into())
    }
}

impl IntoListenAddr for u16 {
    fn into_listen_addr(self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::from((Ipv4Addr::UNSPECIFIED, self)))
    }
}

impl IntoListenAddr for (IpAddr, u16) {
    fn into_listen_addr(self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::from(self))
    }
}

impl IntoListenAddr for (Ipv4Addr, u16) {
    fn into_listen_addr(self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::from(self))
    }
}

impl IntoListenAddr for (Ipv6Addr, u16) {
    fn into_listen_addr(self) -> io::Result<SocketAddr> {
        Ok(SocketAddr::from(self))
    }
}

impl IntoListenAddr for &str {
    fn into_listen_addr(self) -> io::Result<SocketAddr> {
        resolve_first(self, self)
    }
}

impl IntoListenAddr for String {
    fn into_listen_addr(self) -> io::Result<SocketAddr> {
        resolve_first(&self, &self)
    }
}

impl IntoListenAddr for (&str, u16) {
    fn into_listen_addr(self) -> io::Result<SocketAddr> {
        resolve_first(self, format!("{}:{}", self.0, self.1))
    }
}
