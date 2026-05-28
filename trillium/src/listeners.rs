use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
    net::SocketAddr,
    ops::Deref,
    sync::Arc,
};

/// What kind of network binding a [`Listener`] describes: a TCP or QUIC address, a
/// Unix-domain-socket path, or a string descriptor for a kind this enum does not name.
///
/// It is `#[non_exhaustive]`: matching must include a wildcard arm, and new variants may be added
/// in a minor release. An [`Other`](ListenerKind::Other) variant carries a human-readable
/// descriptor for listener kinds this enum does not (yet) name — a producer with a novel transport
/// describes it as a string rather than forcing a downcast no generic reader could use.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ListenerKind {
    /// A TCP listener bound to this address. An inherited listener (e.g. from systemfd/`LISTEN_FD`)
    /// is reported as `Tcp` once adopted, since it is an ordinary TCP listener with a resolvable
    /// local address.
    Tcp(SocketAddr),

    /// A QUIC/UDP listener serving HTTP/3, bound to this address. Always TLS.
    Quic(SocketAddr),

    /// A Unix-domain-socket listener. The path is `None` for an unnamed or abstract socket.
    #[cfg(unix)]
    Unix(Option<std::path::PathBuf>),

    /// A listener kind this enum does not name, described for display purposes only.
    Other(Cow<'static, str>),
}

impl Display for ListenerKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp(addr) | Self::Quic(addr) => Display::fmt(addr, f),
            #[cfg(unix)]
            Self::Unix(Some(path)) => Display::fmt(&path.display(), f),
            #[cfg(unix)]
            Self::Unix(None) => f.write_str("<unnamed unix socket>"),
            Self::Other(descriptor) => f.write_str(descriptor),
        }
    }
}

/// A description of one listener a server is bound to.
///
/// A [`Conn`](crate::Conn) carries the `Listener` it arrived on in its state, and a server exposes
/// the full set on [`Info`](crate::Info) at startup, so a runtime-neutral handler can answer both
/// "what is this server listening on?" and "which listener did this request come from?" — to
/// announce every binding at launch, say, or to tag a log line with the ingress a request arrived
/// on.
///
/// Cloning is a reference-count bump, so stamping it onto every conn is cheap.
///
/// The inner representation is private, so further provenance (e.g. negotiated SNI) can be added in
/// a minor release without a breaking change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listener(Arc<Inner>);

#[derive(Debug, PartialEq, Eq)]
struct Inner {
    kind: ListenerKind,
    secure: bool,
}

impl Listener {
    /// Construct a `Listener` from its kind and whether the listener terminates TLS.
    #[must_use]
    pub fn new(kind: ListenerKind, secure: bool) -> Self {
        Self(Arc::new(Inner { kind, secure }))
    }

    /// Construct a TCP `Listener`.
    #[must_use]
    pub fn tcp(addr: SocketAddr, secure: bool) -> Self {
        Self::new(ListenerKind::Tcp(addr), secure)
    }

    /// Construct a QUIC/HTTP3 `Listener`. QUIC is always TLS.
    #[must_use]
    pub fn quic(addr: SocketAddr) -> Self {
        Self::new(ListenerKind::Quic(addr), true)
    }

    /// Construct a Unix-domain-socket `Listener`. The path is `None` for an unnamed or abstract
    /// socket.
    #[cfg(unix)]
    #[must_use]
    pub fn unix(path: Option<std::path::PathBuf>, secure: bool) -> Self {
        Self::new(ListenerKind::Unix(path), secure)
    }

    /// The kind of listener this describes.
    #[must_use]
    pub fn kind(&self) -> &ListenerKind {
        &self.0.kind
    }

    /// Whether this listener terminates TLS. This reflects the listener's immutable ground truth,
    /// not necessarily the view a client sees: a plaintext listener fronted by a TLS-terminating
    /// proxy reports `false`. For the (mutable, possibly forwarding-header-derived) per-request
    /// view, see [`Conn::is_secure`](crate::Conn::is_secure).
    #[must_use]
    pub fn is_secure(&self) -> bool {
        self.0.secure
    }

    /// The bound socket address, for listener kinds that have one ([`Tcp`](ListenerKind::Tcp) and
    /// [`Quic`](ListenerKind::Quic)). `None` for a Unix socket or an unaddressed kind.
    #[must_use]
    pub fn socket_addr(&self) -> Option<SocketAddr> {
        match self.0.kind {
            ListenerKind::Tcp(addr) | ListenerKind::Quic(addr) => Some(addr),
            _ => None,
        }
    }

    /// The bound port, for listener kinds that have one.
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.socket_addr().map(|addr| addr.port())
    }
}

impl Display for Listener {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0.kind {
            ListenerKind::Tcp(addr) | ListenerKind::Quic(addr) => {
                let scheme = if self.0.secure { "https" } else { "http" };
                write!(f, "{scheme}://{addr}")
            }
            #[cfg(unix)]
            ListenerKind::Unix(Some(path)) => write!(f, "unix:{}", path.display()),
            #[cfg(unix)]
            ListenerKind::Unix(None) => f.write_str("unix:<unnamed>"),
            ListenerKind::Other(descriptor) => f.write_str(descriptor),
        }
    }
}

/// The full set of [`Listener`]s a server is bound to, in registration order.
///
/// Stored in a server's shared state and read back via [`Info::listeners`](crate::Info::listeners)
/// and [`BoundInfo::listeners`](crate::Info). Dereferences to `[Listener]`, so it iterates and
/// slices like a `Vec<Listener>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listeners(pub Vec<Listener>);

impl Deref for Listeners {
    type Target = [Listener];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Vec<Listener>> for Listeners {
    fn from(listeners: Vec<Listener>) -> Self {
        Self(listeners)
    }
}

impl FromIterator<Listener> for Listeners {
    fn from_iter<T: IntoIterator<Item = Listener>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}
