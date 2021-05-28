use async_tls::client::TlsStream;
use async_tls::TlsConnector;
use futures_lite::{AsyncRead, AsyncWrite};
use rustls::{ClientConfig, RootCertStore};
use std::fmt::{self, Debug, Formatter};
use std::io::{Error, ErrorKind, Result};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use trillium::async_trait;

use url::Url;

use crate::ClientTransport;

#[derive(Debug)]
pub enum Rustls<T> {
    Tcp(T),
    Tls(TlsStream<T>),
}

#[derive(Clone)]
pub struct RustlsConfig<Config> {
    pub rustls_config: Arc<ClientConfig>,
    pub tcp_config: Config,
}

impl<Config: Default> Default for RustlsConfig<Config> {
    fn default() -> Self {
        let root_store = match rustls_native_certs::load_native_certs() {
            Ok(certs) => certs,

            Err((Some(best_effort), e)) => {
                log::warn!("rustls native certs soft error, using best effort: {:?}", e);
                best_effort
            }

            Err((_, e)) => {
                log::warn!(
                    "rustls native certs hard error, falling back to webpki roots: {:?}",
                    e
                );
                let mut root_store = RootCertStore::empty();
                root_store.add_server_trust_anchors(&webpki_roots::TLS_SERVER_ROOTS);
                root_store
            }
        };

        let mut config = ClientConfig::new();
        config.root_store = root_store;
        config.into()
    }
}

impl<Config: Default> From<ClientConfig> for RustlsConfig<Config> {
    fn from(rustls_config: ClientConfig) -> Self {
        Self {
            rustls_config: Arc::new(rustls_config),
            tcp_config: Config::default(),
        }
    }
}

impl<Config: Debug> Debug for RustlsConfig<Config> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RustlsConfig")
            .field("rustls_config", &"..")
            .field("tcp_config", &self.tcp_config)
            .finish()
    }
}

impl<T: ClientTransport> AsyncRead for Rustls<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        match &mut *self {
            Rustls::Tcp(t) => Pin::new(t).poll_read(cx, buf),
            Rustls::Tls(t) => Pin::new(t).poll_read(cx, buf),
        }
    }
}

impl<T: ClientTransport> AsyncWrite for Rustls<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        match &mut *self {
            Rustls::Tcp(t) => Pin::new(t).poll_write(cx, buf),
            Rustls::Tls(t) => Pin::new(t).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut *self {
            Rustls::Tcp(t) => Pin::new(t).poll_flush(cx),
            Rustls::Tls(t) => Pin::new(t).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut *self {
            Rustls::Tcp(t) => Pin::new(t).poll_close(cx),
            Rustls::Tls(t) => Pin::new(t).poll_close(cx),
        }
    }
}

#[async_trait]
impl<T: ClientTransport> ClientTransport for Rustls<T> {
    type Config = RustlsConfig<T::Config>;
    fn peer_addr(&self) -> Result<SocketAddr> {
        match self {
            Rustls::Tcp(t) => t.peer_addr(),
            Rustls::Tls(t) => t.get_ref().peer_addr(),
        }
    }

    async fn connect(url: &Url, config: &Self::Config) -> Result<Self> {
        match url.scheme() {
            "https" => {
                let mut http = url.clone();
                http.set_scheme("http").ok();
                http.set_port(url.port_or_known_default()).ok();

                let connector: TlsConnector = Arc::clone(&config.rustls_config).into();

                Ok(Self::Tls(
                    connector
                        .connect(
                            url.domain()
                                .ok_or_else(|| Error::new(ErrorKind::Other, "missing domain"))?,
                            T::connect(&http, &config.tcp_config).await?,
                        )
                        .await
                        .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))?,
                ))
            }
            "http" => Ok(Self::Tcp(T::connect(&url, &config.tcp_config).await?)),

            unknown => Err(Error::new(
                ErrorKind::InvalidInput,
                format!("unknown scheme {}", unknown),
            )),
        }
    }
}
