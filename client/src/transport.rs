use std::fmt::Debug;
use std::io::{ErrorKind, Result};
use std::net::SocketAddr;

use async_net::TcpStream;
use futures_lite::{AsyncRead, AsyncWrite};
use myco::async_trait;
use url::Url;

#[async_trait]
pub trait ClientTransport: Sized + AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static {
    type Config: Debug + Default + Send + Sync + Clone;
    fn peer_addr(&self) -> Result<SocketAddr>;
    async fn connect(url: &Url, config: &Self::Config) -> Result<Self>;
}

#[derive(Default, Debug, Clone)]
pub struct TcpConfig {
    pub nodelay: Option<bool>,
    pub ttl: Option<u32>,
}

#[async_trait]
impl ClientTransport for TcpStream {
    type Config = TcpConfig;
    fn peer_addr(&self) -> Result<SocketAddr> {
        self.peer_addr()
    }

    async fn connect(url: &Url, config: &Self::Config) -> Result<Self> {
        let socket_addrs = url.socket_addrs(|| None)?;
        if url.scheme() != "http" {
            Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("unknown scheme {}", url.scheme()),
            ))
        } else {
            let tcp = Self::connect(&socket_addrs[..]).await?;
            if let Some(nodelay) = config.nodelay {
                tcp.set_nodelay(nodelay)?;
            }
            if let Some(ttl) = config.ttl {
                tcp.set_ttl(ttl)?;
            }
            Ok(tcp)
        }
    }
}
