use crate::{SmolRuntime, SmolTransport};
use async_net::TcpStream;
use std::io::{Error, ErrorKind, Result};
use trillium_server_common::{
    Connector, Transport,
    url::{Host, Url},
};

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

    fn runtime(&self) -> Self::Runtime {
        SmolRuntime::default()
    }

    async fn connect(&self, url: &Url) -> Result<Self::Transport> {
        if url.scheme() != "http" {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("unknown scheme {}", url.scheme()),
            ));
        }

        let port = url
            .port_or_known_default()
            // this should be ok because we already checked that the scheme is http, which has a
            // default port
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, format!("{url} missing port")))?;

        let host = url
            .host()
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, format!("{url} missing host")))?;

        let mut tcp = match host {
            Host::Domain(domain) => Self::Transport::connect((domain, port)).await?,
            Host::Ipv4(ip) => Self::Transport::connect((ip, port)).await?,
            Host::Ipv6(ip) => Self::Transport::connect((ip, port)).await?,
        };

        if let Some(nodelay) = self.nodelay {
            tcp.set_nodelay(nodelay)?;
        }

        if let Some(ttl) = self.ttl {
            tcp.set_ip_ttl(ttl)?;
        }

        Ok(tcp)
    }
}

#[cfg(unix)]
impl Connector for SmolTransport<async_net::unix::UnixStream> {
    type Runtime = SmolRuntime;
    type Transport = Self;

    async fn connect(&self, _url: &Url) -> Result<Self::Transport> {
        Ok(self.clone())
    }

    fn runtime(&self) -> Self::Runtime {
        SmolRuntime::default()
    }
}
