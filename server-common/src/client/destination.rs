use crate::Url;
use smallvec::SmallVec;
use std::{borrow::Cow, io, net::SocketAddr};

/// Everything a [`Connector`](crate::Connector) needs to open one connection: where to dial,
/// whether to wrap it in TLS, and any per-connection ALPN.
///
/// Construct with [`new_with_host`](Destination::new_with_host) — resolve a domain, or dial
/// pre-resolved addresses while validating against the domain — or
/// [`new_with_socket_addrs`](Destination::new_with_socket_addrs) — dial pre-resolved addresses as a
/// bare IP, with no SNI. There is deliberately no `Default`: a destination with neither a host nor
/// any address is unconnectable, so each constructor fills at least one dial source.
#[derive(Debug, Clone)]
pub struct Destination {
    secure: bool,
    host: Option<String>,
    port: u16,
    addrs: SmallVec<[SocketAddr; 4]>,
    /// `None` leaves the connector's configured default ALPN in place; `Some` overrides it for
    /// this connection — including `Some([])`, which advertises no ALPN at all.
    alpn: Option<SmallVec<[Cow<'static, [u8]>; 4]>>,
}

impl Destination {
    /// A destination identified by host name and port.
    ///
    /// With no [`addrs`](Destination::with_addrs) added, the connector resolves `host` itself.
    /// Adding pre-resolved addresses dials those instead while still validating the certificate
    /// against `host`.
    pub fn new_with_host(secure: bool, host: impl Into<String>, port: u16) -> Self {
        Self {
            secure,
            host: Some(host.into()),
            port,
            addrs: SmallVec::new(),
            alpn: None,
        }
    }

    /// A destination identified only by pre-resolved socket addresses: a bare-IP connection with no
    /// SNI. [`Connector::connect_to`](crate::Connector::connect_to) validates TLS against the
    /// address actually dialed; [`Connector::connect_over`](crate::Connector::connect_over), which
    /// does not dial, validates against the first address. The port is taken from the first
    /// address.
    pub fn new_with_socket_addrs(
        secure: bool,
        addrs: impl IntoIterator<Item = SocketAddr>,
    ) -> Self {
        let addrs = addrs.into_iter().collect::<SmallVec<[SocketAddr; 4]>>();
        let port = addrs.first().map_or(0, SocketAddr::port);
        Self {
            secure,
            host: None,
            port,
            addrs,
            alpn: None,
        }
    }

    /// Build a destination from a URL: maps `http`/`https` to plaintext/TLS and extracts the host
    /// and port. An IP-literal host becomes a
    /// [`new_with_socket_addrs`](Destination::new_with_socket_addrs) destination, so it is never
    /// sent to a resolver; a domain becomes a [`new_with_host`](Destination::new_with_host)
    /// destination.
    ///
    /// # Errors
    ///
    /// Returns an error if the scheme is neither `http` nor `https`, or the URL has no host or
    /// port.
    pub fn from_url(url: &Url) -> io::Result<Self> {
        let secure = match url.scheme() {
            "http" => false,
            "https" => true,
            other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("unknown scheme {other}"),
                ));
            }
        };
        let port = url.port_or_known_default().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, format!("{url} missing port"))
        })?;
        match url.host() {
            Some(url::Host::Domain(domain)) => Ok(Self::new_with_host(secure, domain, port)),
            Some(url::Host::Ipv4(ip)) => Ok(Self::new_with_socket_addrs(
                secure,
                [SocketAddr::from((ip, port))],
            )),
            Some(url::Host::Ipv6(ip)) => Ok(Self::new_with_socket_addrs(
                secure,
                [SocketAddr::from((ip, port))],
            )),
            None => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{url} missing host"),
            )),
        }
    }

    /// Reconstruct a URL for this destination, used by the default
    /// [`connect_to`](crate::Connector::connect_to) implementation to fall back to
    /// [`connect`](crate::Connector::connect). Pre-resolved addresses and ALPN are not represented.
    ///
    /// # Errors
    ///
    /// Returns an error if the destination has neither a host nor any address, or the result does
    /// not parse as a URL.
    pub fn to_url(&self) -> io::Result<Url> {
        let scheme = if self.secure { "https" } else { "http" };
        let authority = match &self.host {
            Some(host) => format!("{host}:{}", self.port),
            None => self
                .addrs
                .first()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "destination has neither host nor addresses",
                    )
                })?
                .to_string(),
        };
        Url::parse(&format!("{scheme}://{authority}"))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
    }

    /// Whether this destination should be reached over TLS.
    #[must_use]
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// The host name used for resolution and certificate validation, or `None` for a bare-IP
    /// destination.
    #[must_use]
    pub fn host(&self) -> Option<&str> {
        self.host.as_deref()
    }

    /// The origin port. Pre-resolved [`addrs`](Destination::addrs) carry their own ports; this is
    /// the port used when resolving the host.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The pre-resolved addresses to dial, or an empty slice to resolve the host.
    #[must_use]
    pub fn addrs(&self) -> &[SocketAddr] {
        &self.addrs
    }

    /// The per-connection ALPN override: `Some` advertises exactly these protocols for this
    /// connection (`Some([])` advertises none), `None` leaves the connector's configured default in
    /// place.
    #[must_use]
    pub fn alpn(&self) -> Option<&[Cow<'static, [u8]>]> {
        self.alpn.as_deref()
    }

    /// Set the pre-resolved addresses to dial, replacing any already present.
    #[must_use]
    pub fn with_addrs(mut self, addrs: impl IntoIterator<Item = SocketAddr>) -> Self {
        self.set_addrs(addrs);
        self
    }

    /// Set the pre-resolved addresses to dial, replacing any already present.
    pub fn set_addrs(&mut self, addrs: impl IntoIterator<Item = SocketAddr>) -> &mut Self {
        self.addrs = addrs.into_iter().collect();
        self
    }

    /// Override the ALPN protocols to advertise for this connection, replacing any already present.
    /// An empty iterator advertises no ALPN at all; to instead defer to the connector's configured
    /// default, use [`without_alpn`](Destination::without_alpn).
    #[must_use]
    pub fn with_alpn(mut self, alpn: impl IntoIterator<Item = Cow<'static, [u8]>>) -> Self {
        self.set_alpn(alpn);
        self
    }

    /// Override the ALPN protocols to advertise for this connection, replacing any already present.
    /// An empty iterator advertises no ALPN at all; to instead defer to the connector's configured
    /// default, use [`clear_alpn`](Destination::clear_alpn).
    pub fn set_alpn(&mut self, alpn: impl IntoIterator<Item = Cow<'static, [u8]>>) -> &mut Self {
        self.alpn = Some(alpn.into_iter().collect());
        self
    }

    /// Clear any per-connection ALPN override so this connection uses the connector's configured
    /// default.
    #[must_use]
    pub fn without_alpn(mut self) -> Self {
        self.clear_alpn();
        self
    }

    /// Clear any per-connection ALPN override so this connection uses the connector's configured
    /// default.
    pub fn clear_alpn(&mut self) -> &mut Self {
        self.alpn = None;
        self
    }

    /// Return a copy of this destination with the `secure` flag overridden.
    #[must_use]
    pub fn with_secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }
}
