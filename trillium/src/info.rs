use std::net::SocketAddr;
use trillium_http::{type_set::entry::Entry, ServerConfig, Swansong, TypeSet};

/// This struct represents information about the currently connected
/// server.
///
/// It is passed to [`Handler::init`](crate::Handler::init).

#[derive(Debug, Default)]
pub struct Info(ServerConfig);
impl From<ServerConfig> for Info {
    fn from(value: ServerConfig) -> Self {
        Self(value)
    }
}
impl From<Info> for ServerConfig {
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

impl Info {
    /// Returns the `local_addr` of a bound tcp listener, if such a
    /// thing exists for this server
    pub fn tcp_socket_addr(&self) -> Option<&SocketAddr> {
        self.state()
    }

    /// Returns the `local_addr` of a bound unix listener, if such a
    /// thing exists for this server
    #[cfg(unix)]
    pub fn unix_socket_addr(&self) -> Option<&std::os::unix::net::SocketAddr> {
        self.state()
    }

    /// Borrow a type from the shared state [`TypeSet`] on this `Info`.
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.0.shared_state().get()
    }

    /// Insert a type into the shared state typeset, returning the previous value if any
    pub fn insert_state<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        self.0.shared_state_mut().insert(value)
    }

    /// Mutate a type in the shared state typeset
    pub fn state_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.0.shared_state_mut().get_mut()
    }

    /// Returns an [`Entry`] into the shared state typeset.
    pub fn state_entry<T: Send + Sync + 'static>(&mut self) -> Entry<'_, T> {
        self.0.shared_state_mut().entry()
    }

    /// chainable interface to insert a type into the shared state typeset
    #[must_use]
    pub fn with_state<T: Send + Sync + 'static>(mut self, value: T) -> Self {
        self.insert_state(value);
        self
    }

    /// Borrow the [`Swansong`] graceful shutdown interface for this server
    pub fn swansong(&self) -> &Swansong {
        self.0.swansong()
    }
}
