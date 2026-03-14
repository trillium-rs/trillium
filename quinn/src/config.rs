use crate::{
    connection::QuinnConnection,
    runtime::{SocketTransport, TrilliumRuntime},
};
use std::{io, net::SocketAddr, sync::Arc};
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
        _info: &mut Info,
    ) -> Option<io::Result<Self::Endpoint>> {
        let quinn_runtime = TrilliumRuntime::<S::Runtime, S::UdpTransport>::new(runtime);
        let socket = match std::net::UdpSocket::bind(addr) {
            Ok(s) => s,
            Err(e) => return Some(Err(e)),
        };

        Some(
            quinn::Endpoint::new(
                quinn::EndpointConfig::default(),
                Some(self.0),
                socket,
                quinn_runtime,
            )
            .map(QuinnEndpoint::new),
        )
    }
}

/// A bound quinn QUIC endpoint that accepts and initiates connections.
pub struct QuinnEndpoint(quinn::Endpoint);

impl QuinnEndpoint {
    /// Wrap a quinn endpoint.
    pub(crate) fn new(endpoint: quinn::Endpoint) -> Self {
        Self(endpoint)
    }
}

impl QuicEndpoint for QuinnEndpoint {
    type Connection = QuinnConnection;

    async fn accept(&self) -> Option<Self::Connection> {
        loop {
            let incoming = self.0.accept().await?;
            match incoming.await {
                Ok(connection) => return Some(QuinnConnection::new(connection)),
                Err(e) => log::error!("QUIC accept failed: {e}"),
            }
        }
    }

    async fn connect(&self, addr: SocketAddr, server_name: &str) -> io::Result<Self::Connection> {
        let connection = self
            .0
            .connect(addr, server_name)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?
            .await
            .map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
        Ok(QuinnConnection::new(connection))
    }
}
