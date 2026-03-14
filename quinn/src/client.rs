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
///     ClientQuicConfig::with_webpki_roots(),
/// );
/// ```
pub struct ClientQuicConfig {
    client_config: quinn::ClientConfig,
}

impl Debug for ClientQuicConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientQuicConfig").finish_non_exhaustive()
    }
}

impl ClientQuicConfig {
    /// Build a `ClientQuicConfig` trusting the [WebPKI](https://github.com/rustls/webpki-roots)
    /// root certificates.
    ///
    /// Suitable for connecting to publicly trusted servers. For custom CA trust or client
    /// authentication, use [`from_rustls_client_config`](Self::from_rustls_client_config).
    ///
    /// Requires the `webpki-roots` crate feature.
    #[cfg(feature = "webpki-roots")]
    pub fn with_webpki_roots() -> Self {
        let root_store =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let crypto =
            rustls::ClientConfig::builder_with_provider(crate::crypto_provider::crypto_provider())
                .with_safe_default_protocol_versions()
                .expect("building TLS config with protocol versions")
                .with_root_certificates(root_store)
                .with_no_client_auth();

        Self::from_rustls_client_config(crypto)
    }

    /// Build from a pre-built [`rustls::ClientConfig`].
    ///
    /// `h3` ALPN is added automatically if not already present.
    pub fn from_rustls_client_config(mut tls: rustls::ClientConfig) -> Self {
        if !tls.alpn_protocols.contains(&b"h3".to_vec()) {
            tls.alpn_protocols.push(b"h3".to_vec());
        }
        let quic_tls = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(tls))
            .expect("building QUIC client TLS config");
        Self::from_quinn_client_config(quinn::ClientConfig::new(Arc::new(quic_tls)))
    }

    /// Build from a pre-built [`quinn::ClientConfig`].
    ///
    /// Use this when you need full control over transport parameters or TLS. The caller is
    /// responsible for including `h3` in ALPN protocols.
    pub fn from_quinn_client_config(config: quinn::ClientConfig) -> Self {
        Self {
            client_config: config,
        }
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
        Ok(QuinnEndpoint::new(endpoint))
    }
}
