use async_std::net::TcpStream;
use std::{io::Result, net::SocketAddr};
use trillium_server_common::{async_trait, Connector, Url};

#[derive(Default, Debug, Clone, Copy)]
pub struct ClientConfig {
    pub nodelay: Option<bool>,
    pub ttl: Option<u32>,
}

#[derive(Clone, Debug, Copy)]
pub struct TcpConnector;

#[async_trait]
impl Connector for TcpConnector {
    type Config = ClientConfig;
    type Transport = TcpStream;

    fn peer_addr(transport: &Self::Transport) -> Result<SocketAddr> {
        transport.peer_addr()
    }

    async fn connect(url: &Url, config: &Self::Config) -> Result<Self::Transport> {
        let socket_addrs = url.socket_addrs(|| None)?;
        if url.scheme() != "http" {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unknown scheme {}", url.scheme()),
            ))
        } else {
            let tcp = Self::Transport::connect(&socket_addrs[..]).await?;

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
