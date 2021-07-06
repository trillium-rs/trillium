use async_net::TcpStream;
use futures_lite::Future;
use std::{io::Result, net::SocketAddr};
use trillium_server_common::{async_trait, AsConnector, Connector, Url};

/**
configuration for the tcp Connector
*/
#[derive(Default, Debug, Clone, Copy)]
pub struct ClientConfig {
    /// disable [nagle's algorithm](https://en.wikipedia.org/wiki/Nagle%27s_algorithm)
    pub nodelay: Option<bool>,

    /// set a time to live for the tcp protocol
    pub ttl: Option<u32>,
}

/**
tcp connector for smol based on async_net::TcpStream
*/
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

    fn spawn<Fut>(future: Fut)
    where
        Fut: Future + Send + 'static,
        <Fut as Future>::Output: Send,
    {
        async_global_executor::spawn(future).detach();
    }
}

#[async_trait]
impl<A> Connector for crate::server::Smol<A>
where
    A: AsConnector<TcpConnector> + Clone + Send + Sync + 'static,
{
    type Config = <<A as AsConnector<TcpConnector>>::Connector as Connector>::Config;
    type Transport = <<A as AsConnector<TcpConnector>>::Connector as Connector>::Transport;

    fn peer_addr(transport: &Self::Transport) -> Result<SocketAddr> {
        A::Connector::peer_addr(transport)
    }

    async fn connect(url: &Url, config: &Self::Config) -> Result<Self::Transport> {
        A::Connector::connect(url, config).await
    }

    fn spawn<Fut>(future: Fut)
    where
        Fut: Future + Send + 'static,
        <Fut as Future>::Output: Send,
    {
        async_global_executor::spawn(future).detach();
    }
}
