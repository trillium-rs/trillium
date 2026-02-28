use crate::crypto_provider;
use RustlsClientTransportInner::{Tcp, Tls};
use futures_rustls::{
    TlsConnector,
    client::TlsStream,
    rustls::{
        ClientConfig, ClientConnection, client::danger::ServerCertVerifier, crypto::CryptoProvider,
        pki_types::ServerName,
    },
};
use std::{
    fmt::{self, Debug, Formatter},
    io::{Error, ErrorKind, IoSlice, Result},
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use trillium_server_common::{AsyncRead, AsyncWrite, Connector, Transport, Url};

#[derive(Clone, Debug)]
pub struct RustlsClientConfig(Arc<ClientConfig>);

/// Client configuration for RustlsConnector
#[derive(Clone, Default)]
pub struct RustlsConfig<Config> {
    /// configuration for rustls itself
    pub rustls_config: RustlsClientConfig,

    /// configuration for the inner transport
    pub tcp_config: Config,
}

impl<C: Connector> RustlsConfig<C> {
    /// build a new default rustls config with this tcp config
    pub fn new(rustls_config: impl Into<RustlsClientConfig>, tcp_config: C) -> Self {
        Self {
            rustls_config: rustls_config.into(),
            tcp_config,
        }
    }
}

impl Default for RustlsClientConfig {
    fn default() -> Self {
        Self(Arc::new(default_client_config()))
    }
}

#[cfg(feature = "platform-verifier")]
fn verifier(provider: Arc<CryptoProvider>) -> Arc<dyn ServerCertVerifier> {
    Arc::new(rustls_platform_verifier::Verifier::new(provider).unwrap())
}

#[cfg(not(feature = "platform-verifier"))]
fn verifier(provider: Arc<CryptoProvider>) -> Arc<dyn ServerCertVerifier> {
    let roots = Arc::new(futures_rustls::rustls::RootCertStore::from_iter(
        webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
    ));
    futures_rustls::rustls::client::WebPkiServerVerifier::builder_with_provider(roots, provider)
        .build()
        .unwrap()
}

fn default_client_config() -> ClientConfig {
    let provider = crypto_provider();
    let verifier = verifier(Arc::clone(&provider));

    ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("crypto provider did not support safe default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth()
}

impl From<ClientConfig> for RustlsClientConfig {
    fn from(rustls_config: ClientConfig) -> Self {
        Self(Arc::new(rustls_config))
    }
}

impl From<Arc<ClientConfig>> for RustlsClientConfig {
    fn from(rustls_config: Arc<ClientConfig>) -> Self {
        Self(rustls_config)
    }
}

impl<C: Connector> RustlsConfig<C> {
    /// replace the tcp config
    pub fn with_tcp_config(mut self, config: C) -> Self {
        self.tcp_config = config;
        self
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

impl<C: Connector> Connector for RustlsConfig<C> {
    type Runtime = C::Runtime;
    type Transport = RustlsClientTransport<C::Transport>;

    async fn connect(&self, url: &Url) -> Result<Self::Transport> {
        match url.scheme() {
            "https" => {
                let mut http = url.clone();
                http.set_scheme("http").ok();
                http.set_port(url.port_or_known_default()).ok();

                let connector: TlsConnector = Arc::clone(&self.rustls_config.0).into();
                let domain = url
                    .domain()
                    .and_then(|dns_name| ServerName::try_from(dns_name.to_string()).ok())
                    .ok_or_else(|| Error::new(ErrorKind::Other, "missing domain"))?;

                connector
                    .connect(domain, self.tcp_config.connect(&http).await?)
                    .await
                    .map_err(|e| Error::new(ErrorKind::Other, e.to_string()))
                    .map(Into::into)
            }

            "http" => self.tcp_config.connect(url).await.map(Into::into),

            unknown => Err(Error::new(
                ErrorKind::InvalidInput,
                format!("unknown scheme {unknown}"),
            )),
        }
    }

    fn runtime(&self) -> Self::Runtime {
        self.tcp_config.runtime()
    }
}

#[derive(Debug)]
enum RustlsClientTransportInner<T> {
    Tcp(T),
    Tls(Box<TlsStream<T>>),
}

/// Transport for the rustls connector
///
/// This may represent either an encrypted tls connection or a plaintext
/// connection, depending on the request schema
#[derive(Debug)]
pub struct RustlsClientTransport<T>(RustlsClientTransportInner<T>);
impl<T> From<T> for RustlsClientTransport<T> {
    fn from(value: T) -> Self {
        Self(Tcp(value))
    }
}

impl<T> From<TlsStream<T>> for RustlsClientTransport<T> {
    fn from(value: TlsStream<T>) -> Self {
        Self(Tls(Box::new(value)))
    }
}

impl<C> AsyncRead for RustlsClientTransport<C>
where
    C: AsyncWrite + AsyncRead + Unpin,
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

impl<C> AsyncWrite for RustlsClientTransport<C>
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
            Tls(c) => Pin::new(&mut *c).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut self.0 {
            Tcp(c) => Pin::new(c).poll_flush(cx),
            Tls(c) => Pin::new(&mut *c).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut self.0 {
            Tcp(c) => Pin::new(c).poll_close(cx),
            Tls(c) => Pin::new(&mut *c).poll_close(cx),
        }
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(c) => Pin::new(c).poll_write_vectored(cx, bufs),
            Tls(c) => Pin::new(&mut *c).poll_write_vectored(cx, bufs),
        }
    }
}

impl<T: Transport> Transport for RustlsClientTransport<T> {
    fn peer_addr(&self) -> Result<Option<SocketAddr>> {
        self.as_ref().peer_addr()
    }
}

impl<T> AsRef<T> for RustlsClientTransport<T> {
    fn as_ref(&self) -> &T {
        match &self.0 {
            Tcp(x) => x,
            Tls(x) => x.get_ref().0,
        }
    }
}

impl<T> RustlsClientTransport<T> {
    /// Retrieve the tls [`CommonState`] if this transport is Tls
    pub fn tls_state_mut(&mut self) -> Option<&mut ClientConnection> {
        match &mut self.0 {
            Tls(x) => Some(x.get_mut().1),
            _ => None,
        }
    }

    /// Retrieve the tls [`CommonState`] if this transport is Tls
    pub fn tls_state(&self) -> Option<&ClientConnection> {
        match &self.0 {
            Tls(x) => Some(x.get_ref().1),
            _ => None,
        }
    }
}
