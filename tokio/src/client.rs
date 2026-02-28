use crate::{TokioRuntime, TokioTransport};
use async_compat::Compat;
use std::{
    io::{Error, ErrorKind, Result},
    time::Duration,
};
use tokio::net::TcpStream;
use trillium_server_common::{
    Connector, Transport,
    url::{Host, Url},
};

/// configuration for the tcp Connector
#[derive(Default, Debug, Clone, Copy)]
pub struct ClientConfig {
    /// disable [nagle's algorithm](https://en.wikipedia.org/wiki/Nagle%27s_algorithm)
    /// see [`TcpStream::set_nodelay`] for more info
    pub nodelay: Option<bool>,

    /// time to live for the tcp protocol. set [`TcpStream::set_ttl`] for more info
    pub ttl: Option<u32>,

    /// sets SO_LINGER. I don't really understand this, but see
    /// [`TcpStream::set_linger`] for more info
    pub linger: Option<Option<Duration>>,
}

impl ClientConfig {
    /// constructs a default ClientConfig
    pub const fn new() -> Self {
        Self {
            nodelay: None,
            ttl: None,
            linger: None,
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

    /// chainable setter for linger
    pub const fn with_linger(mut self, linger: Option<Duration>) -> Self {
        self.linger = Some(linger);
        self
    }
}

impl Connector for ClientConfig {
    type Runtime = TokioRuntime;
    type Transport = TokioTransport<Compat<TcpStream>>;

    async fn connect(&self, url: &Url) -> Result<Self::Transport> {
        if url.scheme() != "http" {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                format!("unknown scheme {}", url.scheme()),
            ));
        }

        let host = url
            .host()
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, format!("{url} missing host")))?;

        let port = url
            .port_or_known_default()
            // this should be ok because we already checked that the scheme is http, which has a
            // default port
            .ok_or_else(|| Error::new(ErrorKind::InvalidInput, format!("{url} missing port")))?;

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

        if let Some(dur) = self.linger {
            tcp.set_linger(dur)?;
        }

        Ok(tcp)
    }

    fn runtime(&self) -> Self::Runtime {
        TokioRuntime::default()
    }
}
