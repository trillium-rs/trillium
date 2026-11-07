use crate::{
    config::QuinnEndpoint,
    runtime::{SocketTransport, TrilliumRuntime},
};
use std::{
    fmt::{self, Debug, Formatter},
    io,
    net::SocketAddr,
    sync::Arc,
};
use trillium_server_common::{Connector, QuicClientConfig};

/// Client-side QUIC configuration for HTTP/3, backed by quinn.
///
/// This is a thin factory that creates [`QuinnEndpoint`]s bound to local addresses.
/// The resulting endpoints can both accept and initiate QUIC connections.
///
/// # Construction
///
/// ```rust,ignore
/// use trillium_tokio::ClientConfig;
/// use trillium_quinn::ClientQuicConfig;
///
/// let client = trillium_client::Client::new_with_quic(
///     ClientConfig::default(),
///     ClientQuicConfig::default(),
/// );
/// ```
pub struct ClientQuicConfig {
    client_config: quinn::ClientConfig,
    /// Retained so the bound endpoint can rebuild a per-connection `quinn::ClientConfig` with a
    /// different ALPN (e.g. `doq`) — ALPN is baked into `QuicClientConfig` at construction and
    /// can't be changed on the assembled `client_config`. `None` when built from a pre-assembled
    /// `quinn::ClientConfig` (no rustls config to rebuild from), which disables per-connection
    /// ALPN.
    base_tls: Option<Arc<rustls::ClientConfig>>,
}

impl Debug for ClientQuicConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientQuicConfig").finish_non_exhaustive()
    }
}

impl Default for ClientQuicConfig {
    /// Verifies servers with
    /// [`rustls-platform-verifier`](https://docs.rs/rustls-platform-verifier) when the
    /// `platform-verifier` feature is enabled (the default), and against the
    /// [WebPKI](https://github.com/rustls/webpki-roots) roots compiled into the binary otherwise.
    ///
    /// The platform verifier uses the operating system's verifier and trust store on macOS, iOS,
    /// Windows, and Android. On Linux and other Unix systems it verifies against the system CA
    /// bundle — or, when `SSL_CERT_FILE` or `SSL_CERT_DIR` is set, against only the certificates
    /// those name.
    ///
    /// # Panics
    ///
    /// With the platform verifier on Linux and other Unix systems, panics if no CA certificates
    /// can be loaded: no system bundle is installed, or `SSL_CERT_FILE` and `SSL_CERT_DIR` name no
    /// readable certificates.
    fn default() -> Self {
        Self::from_rustls_client_config(default_tls_config())
    }
}

#[cfg(feature = "platform-verifier")]
fn default_tls_config() -> rustls::ClientConfig {
    use rustls_platform_verifier::BuilderVerifierExt;
    tls_config_builder()
        .with_platform_verifier()
        .unwrap_or_else(|e| {
            panic!(
                "could not build the platform certificate verifier ({e}). If SSL_CERT_FILE or \
                 SSL_CERT_DIR is set, it must name readable PEM certificates; otherwise install \
                 the system CA certificates (often the `ca-certificates` package), or disable \
                 trillium-quinn's `platform-verifier` feature to use the WebPKI roots compiled \
                 into the binary"
            )
        })
        .with_no_client_auth()
}

#[cfg(not(feature = "platform-verifier"))]
fn default_tls_config() -> rustls::ClientConfig {
    webpki_roots_tls_config()
}

fn webpki_roots_tls_config() -> rustls::ClientConfig {
    let root_store =
        rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    tls_config_builder()
        .with_root_certificates(root_store)
        .with_no_client_auth()
}

fn tls_config_builder() -> rustls::ConfigBuilder<rustls::ClientConfig, rustls::WantsVerifier> {
    rustls::ClientConfig::builder_with_provider(crate::crypto_provider::crypto_provider())
        .with_safe_default_protocol_versions()
        .expect("building TLS config with protocol versions")
}

impl ClientQuicConfig {
    /// Build a `ClientQuicConfig` trusting only the
    /// [WebPKI](https://github.com/rustls/webpki-roots) root certificates compiled into the
    /// binary, regardless of the platform's trust configuration.
    ///
    /// [`ClientQuicConfig::default`] is usually what you want. For custom CA trust or client
    /// authentication, use [`from_rustls_client_config`](Self::from_rustls_client_config).
    pub fn with_webpki_roots() -> Self {
        Self::from_rustls_client_config(webpki_roots_tls_config())
    }

    /// Build from a pre-built [`rustls::ClientConfig`].
    ///
    /// `h3` ALPN is added automatically if not already present.
    pub fn from_rustls_client_config(mut tls: rustls::ClientConfig) -> Self {
        if !tls.alpn_protocols.contains(&b"h3".to_vec()) {
            tls.alpn_protocols.push(b"h3".to_vec());
        }
        let tls = Arc::new(tls);
        let quic_tls = quinn::crypto::rustls::QuicClientConfig::try_from(tls.clone())
            .expect("building QUIC client TLS config");
        Self {
            client_config: quinn::ClientConfig::new(Arc::new(quic_tls)),
            base_tls: Some(tls),
        }
    }

    /// Build from a pre-built [`quinn::ClientConfig`].
    ///
    /// Use this when you need full control over transport parameters or TLS. The caller is
    /// responsible for including `h3` in ALPN protocols. Per-connection ALPN override (used by
    /// DNS-over-QUIC) is unavailable on configs built this way — there is no rustls config to
    /// derive an alternate-ALPN config from.
    pub fn from_quinn_client_config(config: quinn::ClientConfig) -> Self {
        Self {
            client_config: config,
            base_tls: None,
        }
    }

    /// Override the quinn [`TransportConfig`](quinn::TransportConfig) governing flow-control
    /// windows, congestion control, GSO, and related transport parameters.
    ///
    /// Composes with any of the `from_*` constructors — construct with TLS credentials, then call
    /// this to replace quinn's default transport configuration. The mirror of
    /// [`QuicConfig::with_transport_config`](crate::QuicConfig::with_transport_config) on the
    /// client side.
    #[must_use]
    pub fn with_transport_config(mut self, transport: Arc<quinn::TransportConfig>) -> Self {
        self.client_config.transport_config(transport);
        self
    }
}

impl<C> QuicClientConfig<C> for ClientQuicConfig
where
    C: Connector,
    C::Runtime: Unpin,
    C::Udp: SocketTransport,
{
    type Endpoint = QuinnEndpoint;

    fn bind(&self, addr: SocketAddr, runtime: &C::Runtime) -> io::Result<Self::Endpoint> {
        let socket = std::net::UdpSocket::bind(addr)?;
        let quinn_runtime = TrilliumRuntime::<C::Runtime, C::Udp>::new(runtime.clone());
        let mut endpoint = quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            None, // client-only, no server config
            socket,
            quinn_runtime,
        )?;
        endpoint.set_default_client_config(self.client_config.clone());
        Ok(QuinnEndpoint::new_client(endpoint, self.base_tls.clone()))
    }
}
