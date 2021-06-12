use async_rustls::{client::TlsStream, webpki::DNSNameRef, TlsConnector};
use rustls::{ClientConfig, RootCertStore};
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    io::{Error, ErrorKind, Result},
    marker::PhantomData,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use trillium_tls_common::{async_trait, AsyncRead, AsyncWrite, Connector, Url};
use RustlsTransportInner::{Tcp, Tls};

/**
this struct provides rustls a trillium client connector implementation
*/
#[derive(Debug, Clone, Copy)]
pub struct RustlsConnector<C>(PhantomData<C>);

#[derive(Debug)]
enum RustlsTransportInner<T> {
    Tcp(T),
    Tls(TlsStream<T>),
}

/**
Transport for the rustls connector

This may represent either an encrypted tls connection or a plaintext
connection, depending on the request schema
*/
#[derive(Debug)]
pub struct RustlsTransport<T>(RustlsTransportInner<T>);

/**
Client configuration for RustlsConnector
*/
#[derive(Clone)]
pub struct RustlsConfig<Config> {
    /// configuration for rustls itself
    pub rustls_config: Arc<ClientConfig>,

    /// configuration for the inner transport
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

impl<C> AsyncRead for RustlsTransport<C>
where
    C: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(c) => Pin::new(c).poll_read(cx, buf),
            Tls(c) => Pin::new(c).poll_read(cx, buf),
        }
    }
}

impl<C> AsyncWrite for RustlsTransport<C>
where
    C: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(c) => Pin::new(c).poll_write(cx, buf),
            Tls(c) => Pin::new(c).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut self.0 {
            Tcp(c) => Pin::new(c).poll_flush(cx),
            Tls(c) => Pin::new(c).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut self.0 {
            Tcp(c) => Pin::new(c).poll_close(cx),
            Tls(c) => Pin::new(c).poll_close(cx),
        }
    }
}

#[async_trait]
impl<C: Connector> Connector for RustlsConnector<C> {
    type Config = RustlsConfig<C::Config>;
    type Transport = RustlsTransport<C::Transport>;
    fn peer_addr(transport: &Self::Transport) -> Result<SocketAddr> {
        match &transport.0 {
            Tcp(c) => C::peer_addr(c),
            Tls(c) => {
                let (x, _) = c.get_ref();
                C::peer_addr(x)
            }
        }
    }

    async fn connect(url: &Url, config: &Self::Config) -> Result<Self::Transport> {
        match url.scheme() {
            "https" => {
                let mut http = url.clone();
                http.set_scheme("http").ok();
                http.set_port(url.port_or_known_default()).ok();

                let connector: TlsConnector = Arc::clone(&config.rustls_config).into();
                let domain = url
                    .domain()
                    .and_then(|dns_name| DNSNameRef::try_from_ascii_str(dns_name).ok())
                    .ok_or_else(|| Error::new(ErrorKind::Other, "missing domain"))?;

                Ok(RustlsTransport(Tls(connector
                    .connect(domain, C::connect(&http, &config.tcp_config).await?)
                    .await
                    .map_err(|e| {
                        Error::new(ErrorKind::Other, e.to_string())
                    })?)))
            }

            "http" => Ok(RustlsTransport(Tcp(
                C::connect(url, &config.tcp_config).await?
            ))),

            unknown => Err(Error::new(
                ErrorKind::InvalidInput,
                format!("unknown scheme {}", unknown),
            )),
        }
    }

    fn spawn<Fut>(future: Fut)
    where
        Fut: Future + Send + 'static,
        <Fut as Future>::Output: Send,
    {
        C::spawn(future);
    }
}
