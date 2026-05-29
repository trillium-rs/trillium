use std::net::SocketAddr;
use trillium_http::{HttpConfig, HttpContext, Swansong, TypeSet, type_set::entry::Entry};

/// This struct represents information about the currently connected
/// server.
///
/// It is passed to [`Handler::init`](crate::Handler::init).

#[derive(Debug, Default)]
pub struct Info(HttpContext);
impl From<HttpContext> for Info {
    fn from(value: HttpContext) -> Self {
        Self(value)
    }
}
impl From<Info> for HttpContext {
    fn from(value: Info) -> Self {
        value.0
    }
}

impl AsRef<TypeSet> for Info {
    fn as_ref(&self) -> &TypeSet {
        self.0.as_ref()
    }
}
impl AsMut<TypeSet> for Info {
    fn as_mut(&mut self) -> &mut TypeSet {
        self.0.as_mut()
    }
}

/// Storage newtype for the full set of bound TCP listener addresses, in registration order. Read
/// via [`Info::tcp_addrs`]; a multi-listener server inserts it during startup, while
/// single-listener servers store only the primary [`SocketAddr`].
#[derive(Debug, Clone, Default)]
#[doc(hidden)]
pub struct BoundTcpAddrs(pub Vec<SocketAddr>);

impl Info {
    /// Returns the `local_addr` of a bound tcp listener, if such a
    /// thing exists for this server
    pub fn tcp_socket_addr(&self) -> Option<&SocketAddr> {
        self.shared_state()
    }

    /// Returns the local addresses of every bound TCP listener, in registration order.
    ///
    /// For a single-listener server this is the one bound address; for a multi-listener server it
    /// is every bound TCP address. Returns an empty slice if no TCP listener has been bound.
    pub fn tcp_addrs(&self) -> &[SocketAddr] {
        if let Some(BoundTcpAddrs(addrs)) = self.shared_state::<BoundTcpAddrs>() {
            addrs.as_slice()
        } else if let Some(addr) = self.shared_state::<SocketAddr>() {
            std::slice::from_ref(addr)
        } else {
            &[]
        }
    }

    /// Returns the `local_addr` of a bound unix listener, if such a
    /// thing exists for this server
    #[cfg(unix)]
    pub fn unix_socket_addr(&self) -> Option<&std::os::unix::net::SocketAddr> {
        self.shared_state()
    }

    /// Borrow a type from the shared state [`TypeSet`] on this `Info`.
    pub fn shared_state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.0.shared_state().get()
    }

    /// Insert a type into the shared state typeset, returning the previous value if any
    pub fn insert_shared_state<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        self.0.shared_state_mut().insert(value)
    }

    /// Mutate a type in the shared state typeset
    pub fn shared_state_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.0.shared_state_mut().get_mut()
    }

    /// Returns an [`Entry`] into the shared state typeset.
    pub fn shared_state_entry<T: Send + Sync + 'static>(&mut self) -> Entry<'_, T> {
        self.0.shared_state_mut().entry()
    }

    /// chainable interface to insert a type into the shared state typeset
    #[must_use]
    pub fn with_shared_state<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.insert_shared_state(value);
        self
    }

    /// borrow the http config
    pub fn config(&self) -> &HttpConfig {
        self.0.config()
    }

    /// mutate the http config
    pub fn config_mut(&mut self) -> &mut HttpConfig {
        self.0.config_mut()
    }

    /// Borrow the [`Swansong`] graceful shutdown interface for this server
    pub fn swansong(&self) -> &Swansong {
        self.0.swansong()
    }
}
