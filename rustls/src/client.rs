use crate::crypto_provider;
use RustlsClientTransportInner::{Tcp, Tls};
#[cfg(feature = "dangerous")]
use futures_rustls::rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified},
    crypto::{verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, UnixTime},
};
use futures_rustls::{
    TlsConnector,
    client::TlsStream,
    rustls::{
        ClientConfig, ClientConnection, RootCertStore,
        client::{WebPkiServerVerifier, danger::ServerCertVerifier},
        crypto::CryptoProvider,
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
use trillium_server_common::{AsyncRead, AsyncWrite, Connector, Destination, Transport, Url};

/// Rustls [`ClientConfig`] wrapper used by [`RustlsConfig`].
///
/// [`RustlsClientConfig::default`] trusts the platform or webpki roots (depending on the
/// `platform-verifier` feature). Use [`RustlsClientConfig::from_root_cert_pem`] to trust a specific
/// private or self-signed certificate instead, or convert an existing [`ClientConfig`] via
/// [`From`].
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
    let roots = Arc::new(RootCertStore::from_iter(
        webpki_roots::TLS_SERVER_ROOTS.iter().cloned(),
    ));
    WebPkiServerVerifier::builder_with_provider(roots, provider)
        .build()
        .unwrap()
}

fn client_config_with_verifier(verifier: Arc<dyn ServerCertVerifier>) -> ClientConfig {
    let mut config = ClientConfig::builder_with_provider(crypto_provider())
        .with_safe_default_protocol_versions()
        .expect("crypto provider did not support safe default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();

    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    config
}

fn default_client_config() -> ClientConfig {
    client_config_with_verifier(verifier(crypto_provider()))
}

impl RustlsClientConfig {
    /// Build a client configuration that trusts exactly the certificate(s) in `pem`.
    ///
    /// Unlike [`RustlsClientConfig::default`], this consults neither the platform trust store nor
    /// the webpki root bundle — the provided roots are the only trust anchors. Server
    /// authentication is otherwise unchanged: certificate chains, signatures, expiry, and server
    /// name are all still verified against these roots. This is the right tool for talking to a
    /// service that presents a private or self-signed certificate.
    ///
    /// The crate's configured crypto provider and default ALPN protocol list (`h2`, `http/1.1`)
    /// are reused.
    ///
    /// # Errors
    ///
    /// Returns an error if `pem` contains no certificates or cannot be parsed, or if the resulting
    /// trust anchors are rejected by the verifier builder.
    pub fn from_root_cert_pem(pem: &[u8]) -> Result<Self> {
        let mut roots = RootCertStore::empty();
        let mut reader = pem;
        for cert in rustls_pemfile::certs(&mut reader) {
            roots.add(cert?).map_err(Error::other)?;
        }

        if roots.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "no certificates found in pem",
            ));
        }

        let verifier =
            WebPkiServerVerifier::builder_with_provider(Arc::new(roots), crypto_provider())
                .build()
                .map_err(Error::other)?;

        Ok(Self(Arc::new(client_config_with_verifier(verifier))))
    }
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

#[cfg(feature = "dangerous")]
#[derive(Debug)]
struct AcceptAnyServerCert(Arc<CryptoProvider>);

#[cfg(feature = "dangerous")]
impl ServerCertVerifier for AcceptAnyServerCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, futures_rustls::rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, futures_rustls::rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, futures_rustls::rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[cfg(feature = "dangerous")]
#[cfg_attr(docsrs, doc(cfg(feature = "dangerous")))]
impl RustlsClientConfig {
    /// Build a client configuration that accepts **any** server certificate without verification.
    ///
    /// ⚠️ This disables server authentication entirely: handshake signatures are still checked,
    /// but the certificate is never validated against any trust anchor, so the connection is
    /// vulnerable to man-in-the-middle attacks. It exists for development against throwaway
    /// self-signed certificates and for `--insecure`-style CLI flags. For talking to a service
    /// with a known private certificate, prefer [`RustlsClientConfig::from_root_cert_pem`], which
    /// keeps authentication intact.
    ///
    /// This constructor is only available with the `dangerous` crate feature enabled, and logs a
    /// warning when called.
    pub fn dangerously_accept_any_cert() -> Self {
        log::warn!(
            "constructing a rustls client config that accepts any server certificate; server \
             authentication is disabled and connections are vulnerable to interception"
        );
        let verifier = Arc::new(AcceptAnyServerCert(crypto_provider()));
        Self(Arc::new(client_config_with_verifier(verifier)))
    }
}

impl<C: Connector> RustlsConfig<C> {
    /// replace the tcp config
    pub fn with_tcp_config(mut self, config: C) -> Self {
        self.tcp_config = config;
        self
    }

    /// Drop `h2` from the ALPN protocol list, forcing HTTP/1.1 over TLS.
    ///
    /// `RustlsConfig::default()` advertises `[h2, http/1.1]` so HTTP/2 is the preferred
    /// protocol when the server supports it. Call this to opt out and pin the connection to
    /// HTTP/1.1.
    #[must_use]
    pub fn without_http2(mut self) -> Self {
        let config = Arc::make_mut(&mut self.rustls_config.0);
        config.alpn_protocols.retain(|p| p != b"h2");
        self
    }
}

impl<Config: Debug> Debug for RustlsConfig<Config> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RustlsConfig")
            .field("rustls_config", &format_args!(".."))
            .field("tcp_config", &self.tcp_config)
            .finish()
    }
}

impl<C: Connector> Connector for RustlsConfig<C> {
    type Runtime = C::Runtime;
    type Transport = RustlsClientTransport<C::Transport>;
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
                .map(Into::into);
        }

        // A per-connection ALPN override replaces the config's default; absent one, the shared
        // config is used as-is. Only clone the (otherwise shared) config when an override is
        // present.
        let rustls_config = if let Some(alpn) = destination.alpn() {
            let mut config = (*self.rustls_config.0).clone();
            config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
            Arc::new(config)
        } else {
            Arc::clone(&self.rustls_config.0)
        };
        let connector: TlsConnector = rustls_config.into();

        // A domain destination's certificate identity (a DNS `ServerName`, sent via SNI) is fixed
        // before the dial, so pre-resolved addresses can't influence validation. A host-less
        // (bare-IP) destination has no SNI and validates against the address actually connected to,
        // so its `IpAddress` server name is derived from the dialed stream below.
        let domain_server_name = destination
            .host()
            .map(|domain| {
                ServerName::try_from(domain.to_owned())
                    .map_err(|e| Error::other(format!("invalid server name {domain:?}: {e}")))
            })
            .transpose()?;

        let stream = self
            .tcp_config
            .connect_to(destination.with_secure(false))
            .await?;

        let server_name = match domain_server_name {
            Some(server_name) => server_name,
            None => {
                let ip = stream
                    .peer_addr()?
                    .ok_or_else(|| Error::other("no peer address for bare-ip destination"))?
                    .ip();
                ServerName::IpAddress(ip.into())
            }
        };

        connector
            .connect(server_name, stream)
            .await
            .map_err(|e| Error::other(e.to_string()))
            .map(Into::into)
    }

    fn runtime(&self) -> Self::Runtime {
        self.tcp_config.runtime()
    }

    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        self.tcp_config.resolve(host, port).await
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

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [std::io::IoSliceMut<'_>],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Tcp(c) => Pin::new(c).poll_read_vectored(cx, bufs),
            Tls(c) => Pin::new(c).poll_read_vectored(cx, bufs),
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

    fn negotiated_alpn(&self) -> Option<std::borrow::Cow<'_, [u8]>> {
        self.tls_state()
            .and_then(|conn| conn.alpn_protocol())
            .map(std::borrow::Cow::Borrowed)
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
    /// Retrieve the tls [`ClientConnection`] if this transport is Tls
    pub fn tls_state_mut(&mut self) -> Option<&mut ClientConnection> {
        match &mut self.0 {
            Tls(x) => Some(x.get_mut().1),
            _ => None,
        }
    }

    /// Retrieve the tls [`ClientConnection`] if this transport is Tls
    pub fn tls_state(&self) -> Option<&ClientConnection> {
        match &self.0 {
            Tls(x) => Some(x.get_ref().1),
            _ => None,
        }
    }
}
