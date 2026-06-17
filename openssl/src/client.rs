use crate::alpn::encode_alpn;
use OpenSslClientTransportInner::{Tcp, Tls};
use async_openssl::SslStream;
use openssl::ssl::{SslConnector, SslMethod};
use std::{
    fmt::{self, Debug, Formatter},
    io::{Error, IoSliceMut, Result},
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use trillium_server_common::{AsyncRead, AsyncWrite, Connector, Destination, Transport, Url};

/// A reference-counted [`SslConnector`] with a sensible default.
///
/// The `Default` impl uses [`SslMethod::tls_client`] and advertises `[h2, http/1.1]` via ALPN.
/// To customize, build a [`SslConnector`] yourself and convert via `From`/`Into`.
#[derive(Clone, Debug)]
pub struct OpenSslClientConfig(Arc<SslConnector>);

impl OpenSslClientConfig {
    /// borrow the inner [`SslConnector`]
    pub fn as_inner(&self) -> &SslConnector {
        &self.0
    }
}

impl Default for OpenSslClientConfig {
    fn default() -> Self {
        let mut builder =
            SslConnector::builder(SslMethod::tls_client()).expect("could not build SslConnector");
        let alpn_wire = encode_alpn(&[b"h2".to_vec(), b"http/1.1".to_vec()]);
        builder
            .set_alpn_protos(&alpn_wire)
            .expect("could not set ALPN protocols");
        Self(Arc::new(builder.build()))
    }
}

impl From<SslConnector> for OpenSslClientConfig {
    fn from(connector: SslConnector) -> Self {
        Self(Arc::new(connector))
    }
}

impl From<Arc<SslConnector>> for OpenSslClientConfig {
    fn from(connector: Arc<SslConnector>) -> Self {
        Self(connector)
    }
}

/// Configuration for the openssl client connector
#[derive(Clone, Default)]
pub struct OpenSslConfig<Config> {
    /// configuration for the inner Connector (usually tcp)
    pub tcp_config: Config,

    /// the openssl client configuration
    pub ssl_config: OpenSslClientConfig,
}

impl<C: Connector> OpenSslConfig<C> {
    /// build a new `OpenSslConfig` from a ssl client configuration and a tcp config
    pub fn new(ssl_config: impl Into<OpenSslClientConfig>, tcp_config: C) -> Self {
        Self {
            tcp_config,
            ssl_config: ssl_config.into(),
        }
    }

    /// replace the tcp config
    #[must_use]
    pub fn with_tcp_config(mut self, config: C) -> Self {
        self.tcp_config = config;
        self
    }
}

impl<Config: Debug> Debug for OpenSslConfig<Config> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenSslConfig")
            .field("tcp_config", &self.tcp_config)
            .field("ssl_config", &self.ssl_config)
            .finish()
    }
}

impl<C> AsRef<C> for OpenSslConfig<C> {
    fn as_ref(&self) -> &C {
        &self.tcp_config
    }
}

impl<C: Connector> Connector for OpenSslConfig<C> {
    type Runtime = C::Runtime;
    type Transport = OpenSslClientTransport<C::Transport>;
    type Udp = C::Udp;

    async fn connect(&self, url: &Url) -> Result<Self::Transport> {
        self.connect_to(Destination::from_url(url)?).await
    }

    async fn connect_to(&self, destination: Destination) -> Result<Self::Transport> {
        if !destination.secure() {
            return self
                .tcp_config
                .connect_to(destination)
                .await
                .map(|t| OpenSslClientTransport(Tcp(t)));
        }

        // OpenSSL's `into_ssl` requires a domain for SNI; a bare-IP destination has none.
        let mut ssl = self
            .ssl_config
            .as_inner()
            .configure()
            .map_err(Error::other)?
            .into_ssl(
                destination
                    .host()
                    .ok_or_else(|| Error::other("missing domain"))?,
            )
            .map_err(Error::other)?;

        // A per-connection ALPN override replaces the connector's default; absent one, the
        // configured default stays in place.
        if let Some(alpn) = destination.alpn() {
            ssl.set_alpn_protos(&encode_alpn(alpn))
                .map_err(Error::other)?;
        }

        let inner = self
            .tcp_config
            .connect_to(destination.with_secure(false))
            .await?;
        let mut stream = SslStream::new(ssl, inner).map_err(Error::other)?;
        Pin::new(&mut stream)
            .connect()
            .await
            .map_err(Error::other)?;
        Ok(OpenSslClientTransport(Tls(Box::new(stream))))
    }

    fn runtime(&self) -> Self::Runtime {
        self.tcp_config.runtime()
    }

    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        self.tcp_config.resolve(host, port).await
    }
}

#[derive(Debug)]
enum OpenSslClientTransportInner<T: Unpin> {
    Tcp(T),
    Tls(Box<SslStream<T>>),
}

/// Transport for the openssl connector
///
/// May represent either an encrypted tls connection or a plaintext connection,
/// depending on the request scheme.
#[derive(Debug)]
pub struct OpenSslClientTransport<T: Unpin>(OpenSslClientTransportInner<T>);

impl<T: Unpin> OpenSslClientTransport<T> {
    /// Borrow the underlying [`SslStream`] if this transport is TLS.
    pub fn as_tls(&self) -> Option<&SslStream<T>> {
        match &self.0 {
            Tcp(_) => None,
            Tls(s) => Some(s),
        }
    }
}

impl<T: Unpin> AsRef<T> for OpenSslClientTransport<T> {
    fn as_ref(&self) -> &T {
        match &self.0 {
            Tcp(t) => t,
            Tls(s) => s.get_ref(),
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncRead for OpenSslClientTransport<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_read(cx, buf),
            Tls(s) => Pin::new(&mut **s).poll_read(cx, buf),
        }
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_read_vectored(cx, bufs),
            Tls(s) => Pin::new(&mut **s).poll_read_vectored(cx, bufs),
        }
    }
}

impl<T: AsyncRead + AsyncWrite + Unpin> AsyncWrite for OpenSslClientTransport<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_write(cx, buf),
            Tls(s) => Pin::new(&mut **s).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_flush(cx),
            Tls(s) => Pin::new(&mut **s).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        match &mut self.0 {
            Tcp(t) => Pin::new(t).poll_close(cx),
            Tls(s) => Pin::new(&mut **s).poll_close(cx),
        }
    }
}

impl<T: Transport> Transport for OpenSslClientTransport<T> {
    fn peer_addr(&self) -> Result<Option<SocketAddr>> {
        self.as_ref().peer_addr()
    }

    fn negotiated_alpn(&self) -> Option<std::borrow::Cow<'_, [u8]>> {
        match &self.0 {
            Tcp(_) => None,
            Tls(s) => s
                .ssl()
                .selected_alpn_protocol()
                .map(std::borrow::Cow::Borrowed),
        }
    }
}
