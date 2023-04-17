use crate::AsyncStdTransport;
use async_std::net::TcpStream;
use std::{future::Future, io::Result};
use trillium_server_common::{async_trait, Connector, Url};

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

#[async_trait]
impl Connector for ClientConfig {
    type Transport = AsyncStdTransport<TcpStream>;

    async fn connect(&self, url: &Url) -> Result<Self::Transport> {
        let socket_addrs = url.socket_addrs(|| None)?;

        if url.scheme() != "http" {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unknown scheme {}", url.scheme()),
            ))
        } else {
            let tcp = TcpStream::connect(&socket_addrs[..]).await?;

            if let Some(nodelay) = self.nodelay {
                tcp.set_nodelay(nodelay)?;
            }

            if let Some(ttl) = self.ttl {
                tcp.set_ttl(ttl)?;
            }

            Ok(AsyncStdTransport::from(tcp))
        }
    }

    fn spawn<Fut: Future<Output = ()> + Send + 'static>(&self, fut: Fut) {
        async_std::task::spawn(fut);
    }
}
