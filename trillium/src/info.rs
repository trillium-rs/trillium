use std::{
    fmt::{Display, Formatter, Result},
    net::SocketAddr,
};
use trillium_http::TypeSet;

const DEFAULT_SERVER_DESCRIPTION: &str = concat!("trillium v", env!("CARGO_PKG_VERSION"));

/// This struct represents information about the currently connected
/// server.
///
/// It is passed to [`Handler::init`](crate::Handler::init).

#[derive(Debug)]
pub struct Info {
    server_description: String,
    listener_description: String,
    tcp_socket_addr: Option<SocketAddr>,
    state: TypeSet,
}

impl Default for Info {
    fn default() -> Self {
        Self {
            server_description: DEFAULT_SERVER_DESCRIPTION.into(),
            listener_description: String::new(),
            tcp_socket_addr: None,
            state: TypeSet::new(),
        }
    }
}

impl Display for Info {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        f.write_fmt(format_args!(
            "{} listening on {}",
            self.server_description(),
            self.listener_description(),
        ))
    }
}

impl Info {
    /// Returns a user-displayable description of the server. This
    /// might be a string like "trillium x.y.z (trillium-tokio x.y.z)" or "my
    /// special application".
    pub fn server_description(&self) -> &str {
        &self.server_description
    }

    /// Returns a user-displayable string description of the location
    /// or port the listener is bound to, potentially as a url. Do not
    /// rely on the format of this string, as it will vary between
    /// server implementations and is intended for user
    /// display. Instead, use [`Info::tcp_socket_addr`] for any
    /// processing.
    pub fn listener_description(&self) -> &str {
        &self.listener_description
    }

    /// Returns the `local_addr` of a bound tcp listener, if such a
    /// thing exists for this server
    pub const fn tcp_socket_addr(&self) -> Option<&SocketAddr> {
        self.tcp_socket_addr.as_ref()
    }

    /// obtain a mutable borrow of the server description, suitable
    /// for appending information or replacing it
    pub fn server_description_mut(&mut self) -> &mut String {
        &mut self.server_description
    }

    /// obtain a mutable borrow of the listener description, suitable
    /// for appending information or replacing it
    pub fn listener_description_mut(&mut self) -> &mut String {
        &mut self.listener_description
    }

    /// borrow the [`TypeSet`] on this `Info`. This can be useful for passing initialization data
    /// between handlers
    #[allow(clippy::missing_const_for_fn)] // Info isn't useful in a const context
    pub fn state(&self) -> &TypeSet {
        &self.state
    }

    /// attempt to mutably borrow the [`TypeSet`] on this `Info`.
    pub fn state_mut(&mut self) -> &mut TypeSet {
        &mut self.state
    }
}

impl AsRef<TypeSet> for Info {
    fn as_ref(&self) -> &TypeSet {
        self.state()
    }
}

impl AsMut<TypeSet> for Info {
    fn as_mut(&mut self) -> &mut TypeSet {
        self.state_mut()
    }
}

impl From<&str> for Info {
    fn from(description: &str) -> Self {
        Self {
            server_description: String::from(DEFAULT_SERVER_DESCRIPTION),
            listener_description: String::from(description),
            ..Self::default()
        }
    }
}

impl From<SocketAddr> for Info {
    fn from(socket_addr: SocketAddr) -> Self {
        let mut info = Self {
            server_description: String::from(DEFAULT_SERVER_DESCRIPTION),
            listener_description: socket_addr.to_string(),
            tcp_socket_addr: Some(socket_addr),
            ..Self::default()
        };

        info.state_mut().insert(socket_addr);
        info
    }
}

#[cfg(unix)]
impl From<std::os::unix::net::SocketAddr> for Info {
    fn from(s: std::os::unix::net::SocketAddr) -> Self {
        let mut info = Self {
            server_description: String::from(DEFAULT_SERVER_DESCRIPTION),
            listener_description: format!("{s:?}"),
            ..Self::default()
        };

        info.state_mut().insert(s);
        info
    }
}
