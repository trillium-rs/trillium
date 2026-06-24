use crate::{SmolRuntime, SmolTransport};
use async_net::TcpStream;
use std::{
    io::{Error, ErrorKind, Result},
    net::SocketAddr,
};
use trillium_server_common::{Connector, Destination, Transport, url::Url};

/// configuration for the tcp Connector
#[derive(Default, Debug, Clone, Copy)]
pub struct ClientConfig {
    /// disable [nagle's algorithm](https://en.wikipedia.org/wiki/Nagle%27s_algorithm)
    pub nodelay: Option<bool>,

    /// set a time to live for the tcp protocol
    pub ttl: Option<u32>,
}

impl ClientConfig {
    /// constructs a default ClientConfig
    pub const fn new() -> Self {
        Self {
            nodelay: None,
            ttl: None,
        }
    }

    /// chainable setter to set default nodelay
    pub const fn with_nodelay(mut self, nodelay: bool) -> Self {
        self.nodelay = Some(nodelay);
        self
    }

    /// chainable setter for ip ttl
    pub const fn with_ttl(mut self, ttl: u32) -> Self {
        self.ttl = Some(ttl);
        self
    }
}

impl Connector for ClientConfig {
    type Runtime = SmolRuntime;
    type Transport = SmolTransport<TcpStream>;
    type Udp = crate::SmolUdpSocket;

    fn runtime(&self) -> Self::Runtime {
        SmolRuntime::default()
    }

    async fn connect(&self, url: &Url) -> Result<Self::Transport> {
        self.connect_to(Destination::from_url(url)?).await
    }

    async fn connect_to(&self, destination: Destination) -> Result<Self::Transport> {
        if destination.secure() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "this connector does not support TLS",
            ));
        }

        let addrs = destination.addrs();
        let mut tcp = if addrs.is_empty() {
            let host = destination.host().ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidInput,
                    "destination has neither host nor addresses",
                )
            })?;
            Self::Transport::connect((host, destination.port())).await?
        } else {
            Self::Transport::connect(addrs).await?
        };

        if let Some(nodelay) = self.nodelay {
            tcp.set_nodelay(nodelay)?;
        }

        if let Some(ttl) = self.ttl {
            tcp.set_ip_ttl(ttl)?;
        }

        Ok(tcp)
    }

    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        async_net::resolve((host, port)).await
    }
}

/// A [`Connector`] that dials a fixed Unix domain socket path.
///
/// Every connection opens a fresh `UnixStream` to `path`, so a single `UnixClientConfig` is safe to
/// share across a pooled [`Client`](https://docs.trillium.rs/trillium_client/struct.Client.html) making
/// concurrent requests. The request URL's host and port are ignored — the socket path is the
/// address — though the URL still supplies request metadata such as the `Host` header.
///
/// This handles only the single-socket case. To route different requests to different upstreams (a
/// mix of TCP and Unix sockets, or several Unix sockets), compose connectors behind a routing
/// [`Connector`] that dispatches on the [`Destination`].
#[cfg(unix)]
#[derive(Clone, Debug)]
pub struct UnixClientConfig {
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl UnixClientConfig {
    /// Construct a `UnixClientConfig` that dials the provided Unix domain socket path.
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }

    async fn dial(&self) -> Result<SmolTransport<async_net::unix::UnixStream>> {
        SmolTransport::connect_unix(&self.path).await
    }
}

#[cfg(unix)]
impl Connector for UnixClientConfig {
    type Runtime = SmolRuntime;
    type Transport = SmolTransport<async_net::unix::UnixStream>;
    type Udp = ();

    async fn connect(&self, url: &Url) -> Result<Self::Transport> {
        if url.scheme() == "https" {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "this connector does not support TLS",
            ));
        }
        self.dial().await
    }

    async fn connect_to(&self, destination: Destination) -> Result<Self::Transport> {
        if destination.secure() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "this connector does not support TLS",
            ));
        }
        self.dial().await
    }

    fn runtime(&self) -> Self::Runtime {
        SmolRuntime::default()
    }

    async fn resolve(&self, _host: &str, _port: u16) -> Result<Vec<SocketAddr>> {
        Ok(vec![])
    }
}
