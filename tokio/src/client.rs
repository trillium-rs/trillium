use async_compat::Compat;
use std::{io::Result, net::SocketAddr, time::Duration};
use tokio::net::TcpStream;
use trillium_server_common::{async_trait, Connector, Url};

#[derive(Default, Debug, Clone, Copy)]
pub struct ClientConfig {
    pub nodelay: Option<bool>,
    pub ttl: Option<u32>,
    pub linger: Option<Duration>,
}

#[derive(Clone, Debug, Copy)]
pub struct TcpConnector;

#[async_trait]
impl Connector for TcpConnector {
    type Config = ClientConfig;
    type Transport = Compat<TcpStream>;

    fn peer_addr(transport: &Self::Transport) -> Result<SocketAddr> {
        transport.get_ref().peer_addr()
    }

    async fn connect(url: &Url, config: &Self::Config) -> Result<Self::Transport> {
        let socket_addrs = url.socket_addrs(|| None)?;
        if url.scheme() != "http" {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unknown scheme {}", url.scheme()),
            ))
        } else {
            let tcp = TcpStream::connect(&socket_addrs[..]).await?;

            if let Some(nodelay) = config.nodelay {
                tcp.set_nodelay(nodelay)?;
            }

            if let Some(ttl) = config.ttl {
                tcp.set_ttl(ttl)?;
            }

            if let Some(dur) = config.linger {
                tcp.set_linger(Some(dur))?;
            }

            Ok(Compat::new(tcp))
        }
    }
}
