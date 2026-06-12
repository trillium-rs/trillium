use crate::{AsyncStdRuntime, AsyncStdTransport};
use async_std::net::{TcpStream, ToSocketAddrs};
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
    type Runtime = AsyncStdRuntime;
    type Transport = AsyncStdTransport<TcpStream>;
    type Udp = crate::AsyncStdUdpSocket;

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

    fn runtime(&self) -> Self::Runtime {
        AsyncStdRuntime::default()
    }

    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        (host, port).to_socket_addrs().await.map(Iterator::collect)
    }
}
