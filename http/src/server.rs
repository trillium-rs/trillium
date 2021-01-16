//! Process HTTP connections on the server.

// use async_std::future::{timeout, Future, TimeoutError};
// use async_std::io::{self, Read, Write};
use crate::{Conn, Error, Upgrade};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::future::Future;
use std::time::Duration;

/// Configure the server.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Timeout to handle headers. Defaults to 60s.
    headers_timeout: Option<Duration>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            headers_timeout: Some(Duration::from_secs(60)),
        }
    }
}

/// struct for server
#[derive(Debug)]
pub struct Server {
    opts: ServerOptions,
}

/// An enum that represents whether the server should accept a subsequent request
#[derive(Debug)]
pub enum ConnectionStatus<RW> {
    /// The server should not accept another request
    Close,

    /// The server may accept another request
    KeepAlive(RW, Option<Vec<u8>>),

    /// upgrade
    Upgrade(Upgrade<RW>),
}

impl Server {
    /// builds a new server
    pub fn new() -> Self {
        Self {
            opts: Default::default(),
        }
    }

    /// with opts
    pub fn with_opts(mut self, opts: ServerOptions) -> Self {
        self.opts = opts;
        self
    }

    /// accept in a loop
    pub async fn accept<RW, F, Fut>(&self, rw: RW, f: F) -> crate::Result<Option<Upgrade<RW>>>
    where
        RW: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
        F: Fn(Conn<RW>) -> Fut,
        Fut: Future<Output = Conn<RW>> + Send,
    {
        log::info!("new connection");
        let mut status = ConnectionStatus::KeepAlive(rw, None);

        loop {
            match status {
                ConnectionStatus::Upgrade(upgrade) => return Ok(Some(upgrade)),
                ConnectionStatus::Close => return Ok(None),
                ConnectionStatus::KeepAlive(rw, bytes) => {
                    status = self.accept_one(rw, bytes, &f).await?;
                }
            }
        }
    }

    /// accept one request
    pub async fn accept_one<RW, F, Fut>(
        &self,
        rw: RW,
        bytes: Option<Vec<u8>>,
        f: &F,
    ) -> crate::Result<ConnectionStatus<RW>>
    where
        RW: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
        F: Fn(Conn<RW>) -> Fut,
        Fut: Future<Output = Conn<RW>> + Send,
    {
        log::info!("new request");
        let conn = match Conn::new(rw, bytes).await {
            Err(Error::ClosedByClient) => {
                log::trace!("connection closed by client");
                return Ok(ConnectionStatus::Close);
            }
            Err(e) => return Err(e),
            Ok(conn) => conn,
        };

        f(conn).await.encode().await
    }
}
