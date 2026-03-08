use crate::{
    connection::QuinnConnection,
    runtime::{SocketTransport, TrilliumRuntime},
};
use std::{io, net::SocketAddr, sync::Arc};
use trillium_server_common::{QuicBinding, QuicConfig as QuicConfigTrait, Server};

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
    /// Build a QUIC config from a single PEM-encoded certificate chain and private key.
    pub fn from_single_cert(cert_pem: &[u8], key_pem: &[u8]) -> Self {
        let certs: Vec<_> = rustls_pemfile::certs(&mut io::BufReader::new(cert_pem))
            .collect::<Result<_, _>>()
            .expect("parsing certificate PEM");

        let key = rustls_pemfile::private_key(&mut io::BufReader::new(key_pem))
            .expect("parsing private key PEM")
            .expect("no private key found in PEM");

        let mut tls_config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("building TLS config");

        tls_config.alpn_protocols = vec![b"h3".to_vec()];

        let quic_tls = quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(tls_config))
            .expect("building QUIC TLS config");

        Self(quinn::ServerConfig::with_crypto(Arc::new(quic_tls)))
    }

    /// Build a QUIC config from a pre-built [`quinn::ServerConfig`].
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
    type Binding = QuinnBinding;

    fn bind(self, addr: SocketAddr, runtime: S::Runtime) -> Option<io::Result<Self::Binding>> {
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
            .map(QuinnBinding),
        )
    }
}

/// A bound quinn QUIC endpoint that accepts connections.
pub struct QuinnBinding(quinn::Endpoint);

impl QuicBinding for QuinnBinding {
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
}
