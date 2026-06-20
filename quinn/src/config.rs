use crate::{
    connection::QuinnConnection,
    runtime::{SocketTransport, TrilliumRuntime},
};
use rustls::server::ResolvesServerCert;
use std::{
    borrow::Cow,
    collections::HashMap,
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
};
use trillium_server_common::{Info, QuicConfig as QuicConfigTrait, QuicEndpoint, Server};

/// User-facing QUIC configuration backed by quinn.
///
/// Constructed with TLS credentials and passed to
/// [`Config::with_quic`](trillium_server_common::Config::with_quic).
/// The runtime and UDP transport types are inferred from the server.
///
/// ```rust,ignore
/// trillium_tokio::config()
///     .with_quic(trillium_quinn::QuicConfig::from_single_cert(&cert_pem, &key_pem))
///     .run(handler);
/// ```
pub struct QuicConfig(quinn::ServerConfig);

impl QuicConfig {
    /// Build a `QuicConfig` from a single PEM-encoded certificate chain and private key.
    ///
    /// Automatically configures ALPN for HTTP/3 (`h3`). For a custom TLS setup, use
    /// [`from_rustls_server_config`](Self::from_rustls_server_config).
    pub fn from_single_cert(cert_pem: &[u8], key_pem: &[u8]) -> Self {
        let certs: Vec<_> = rustls_pemfile::certs(&mut io::BufReader::new(cert_pem))
            .collect::<Result<_, _>>()
            .expect("parsing certificate PEM");

        let key = rustls_pemfile::private_key(&mut io::BufReader::new(key_pem))
            .expect("parsing private key PEM")
            .expect("no private key found in PEM");

        let mut tls_config =
            rustls::ServerConfig::builder_with_provider(crate::crypto_provider::crypto_provider())
                .with_safe_default_protocol_versions()
                .expect("building TLS config with protocol versions")
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .expect("building TLS config");

        tls_config.alpn_protocols = vec![b"h3".to_vec()];

        let quic_tls = quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(tls_config))
            .expect("building QUIC TLS config");

        Self(quinn::ServerConfig::with_crypto(Arc::new(quic_tls)))
    }

    /// Construct from a pre-built [`rustls::ServerConfig`].
    ///
    /// Use this when you need a custom TLS setup (client authentication, custom crypto
    /// provider, etc.). HTTP/3 ALPN (`h3`) is added automatically if not already present.
    pub fn from_rustls_server_config(tls_config: rustls::ServerConfig) -> Self {
        let mut tls_config = tls_config;
        if !tls_config.alpn_protocols.contains(&b"h3".to_vec()) {
            tls_config.alpn_protocols.push(b"h3".to_vec());
        }
        let quic_tls = quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(tls_config))
            .expect("building QUIC TLS config");
        Self(quinn::ServerConfig::with_crypto(Arc::new(quic_tls)))
    }

    /// Construct from a pre-built [`quinn::ServerConfig`].
    ///
    /// Use this when you also need to customize quinn transport parameters. The caller is
    /// responsible for configuring ALPN protocols (must include `h3` to support HTTP/3).
    /// For custom TLS only, prefer [`from_rustls_server_config`](Self::from_rustls_server_config).
    pub fn from_quinn_server_config(config: quinn::ServerConfig) -> Self {
        Self(config)
    }

    /// Override the quinn [`TransportConfig`](quinn::TransportConfig) governing flow-control
    /// windows, send fairness, congestion control, GSO, and related transport parameters.
    ///
    /// Composes with any of the `from_*` constructors — construct with TLS credentials, then call
    /// this to replace quinn's default transport configuration.
    #[must_use]
    pub fn with_transport_config(mut self, transport: Arc<quinn::TransportConfig>) -> Self {
        self.0.transport_config(transport);
        self
    }

    /// Build a `QuicConfig` from a [`rustls::server::ResolvesServerCert`] cert resolver.
    ///
    /// Use this to bring your own dynamic certificate source — for example, an ACME integration
    /// that rotates certificates over time. The resolver is consulted on every new connection,
    /// so renewals take effect immediately without rebuilding the QUIC server config.
    ///
    /// If the resolver returns `None` for a given `ClientHello` (e.g. before the first
    /// certificate has been obtained), the TLS handshake fails and the connection is rejected.
    /// This makes it safe to bind the endpoint before any certificate is available.
    ///
    /// Automatically configures ALPN for HTTP/3 (`h3`).
    pub fn from_cert_resolver(resolver: Arc<dyn ResolvesServerCert>) -> Self {
        let tls_config =
            rustls::ServerConfig::builder_with_provider(crate::crypto_provider::crypto_provider())
                .with_safe_default_protocol_versions()
                .expect("building TLS config with protocol versions")
                .with_no_client_auth()
                .with_cert_resolver(resolver);
        Self::from_rustls_server_config(tls_config)
    }
}

impl<S> QuicConfigTrait<S> for QuicConfig
where
    S: Server,
    S::Runtime: Unpin,
    S::UdpTransport: SocketTransport,
{
    type Endpoint = QuinnEndpoint;

    fn bind(
        self,
        addr: SocketAddr,
        runtime: S::Runtime,
        info: &mut Info,
    ) -> Option<io::Result<Self::Endpoint>> {
        let socket = match std::net::UdpSocket::bind(addr) {
            Ok(s) => s,
            Err(e) => return Some(Err(e)),
        };
        Some(<Self as QuicConfigTrait<S>>::bind_with_socket(
            self, socket, runtime, info,
        ))
    }

    fn bind_with_socket(
        self,
        socket: std::net::UdpSocket,
        runtime: S::Runtime,
        _info: &mut Info,
    ) -> io::Result<Self::Endpoint> {
        let quinn_runtime = TrilliumRuntime::<S::Runtime, S::UdpTransport>::new(runtime);
        quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            Some(self.0),
            socket,
            quinn_runtime,
        )
        .map(QuinnEndpoint::new)
    }
}

/// A bound quinn QUIC endpoint that accepts and initiates connections.
pub struct QuinnEndpoint {
    endpoint: quinn::Endpoint,
    /// The rustls config the per-connection-ALPN configs are derived from, when this endpoint was
    /// bound from a [`ClientQuicConfig`](crate::ClientQuicConfig) built from a rustls config.
    /// `None` for server-bound endpoints and client configs built from a raw
    /// `quinn::ClientConfig`.
    base_tls: Option<Arc<rustls::ClientConfig>>,
    /// Per-ALPN `quinn::ClientConfig`s derived from `base_tls`, keyed by the ALPN protocol list.
    /// The set is small and closed (`h3`, `doq`), so caching avoids rebuilding the crypto config
    /// on every connection.
    alpn_configs: Mutex<HashMap<Vec<Vec<u8>>, quinn::ClientConfig>>,
}

impl QuinnEndpoint {
    /// Wrap a quinn endpoint with no rustls config retained (server-bound, or a client built from a
    /// pre-assembled `quinn::ClientConfig`).
    pub(crate) fn new(endpoint: quinn::Endpoint) -> Self {
        Self {
            endpoint,
            base_tls: None,
            alpn_configs: Mutex::new(HashMap::new()),
        }
    }

    /// Wrap a client endpoint, retaining `base_tls` so per-connection ALPN configs can be derived.
    pub(crate) fn new_client(
        endpoint: quinn::Endpoint,
        base_tls: Option<Arc<rustls::ClientConfig>>,
    ) -> Self {
        Self {
            endpoint,
            base_tls,
            alpn_configs: Mutex::new(HashMap::new()),
        }
    }

    /// Build (and cache) a `quinn::ClientConfig` advertising exactly `alpn`, derived from the
    /// retained rustls config. Returns `None` when no rustls config was retained.
    fn client_config_for_alpn(
        &self,
        alpn: &[Cow<'static, [u8]>],
    ) -> io::Result<Option<quinn::ClientConfig>> {
        let Some(base) = &self.base_tls else {
            return Ok(None);
        };
        let key: Vec<Vec<u8>> = alpn.iter().map(|a| a.to_vec()).collect();

        let mut cache = self.alpn_configs.lock().unwrap();
        if let Some(config) = cache.get(&key) {
            return Ok(Some(config.clone()));
        }

        let mut tls = (**base).clone();
        tls.alpn_protocols = key.clone();
        let quic_tls = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(tls))
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let config = quinn::ClientConfig::new(Arc::new(quic_tls));
        cache.insert(key, config.clone());
        Ok(Some(config))
    }
}

impl QuicEndpoint for QuinnEndpoint {
    type Connection = QuinnConnection;

    async fn accept(&self) -> Option<Self::Connection> {
        loop {
            let incoming = self.endpoint.accept().await?;
            match incoming.await {
                Ok(connection) => return Some(QuinnConnection::new(connection)),
                Err(e) => log::error!("QUIC accept failed: {e}"),
            }
        }
    }

    async fn connect(&self, addr: SocketAddr, server_name: &str) -> io::Result<Self::Connection> {
        let connection = self
            .endpoint
            .connect(addr, server_name)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
        Ok(QuinnConnection::new(connection))
    }

    async fn connect_with_alpn(
        &self,
        addr: SocketAddr,
        server_name: &str,
        alpn: &[Cow<'static, [u8]>],
    ) -> io::Result<Self::Connection> {
        // Empty ALPN, or no retained rustls config to rebuild from, falls back to the endpoint's
        // default client config.
        let Some(config) = (!alpn.is_empty())
            .then(|| self.client_config_for_alpn(alpn))
            .transpose()?
            .flatten()
        else {
            return self.connect(addr, server_name).await;
        };

        let connection = self
            .endpoint
            .connect_with(config, addr, server_name)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
        Ok(QuinnConnection::new(connection))
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.endpoint.local_addr()
    }
}
