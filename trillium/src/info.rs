use std::{fmt::Display, net::SocketAddr};

/**
This struct represents information about the currently connected
server.

It is passed to [`Handler::init`].
*/

#[derive(Debug, Clone)]
pub struct Info {
    description: String,
    tcp_socket_addr: Option<SocketAddr>,
}

impl Display for Info {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.description())
    }
}

impl Info {
    /// Returns a user-displayable string description of the
    /// server. Do not rely on the format of this string
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the local_addr of a bound tcp listener, if such a
    /// thing exists for this server
    pub fn tcp_socket_addr(&self) -> Option<&SocketAddr> {
        self.tcp_socket_addr.as_ref()
    }
}

impl From<&'static str> for Info {
    fn from(description: &'static str) -> Self {
        Self {
            description: String::from(description),
            tcp_socket_addr: None,
        }
    }
}

impl From<SocketAddr> for Info {
    fn from(socket_addr: SocketAddr) -> Self {
        Self {
            description: socket_addr.to_string(),
            tcp_socket_addr: Some(socket_addr),
        }
    }
}
