use crate::{
    connection::QuinnConnection,
    runtime::{SocketTransport, TrilliumRuntime},
};
use futures_lite::{AsyncWriteExt, io};
use std::{
    fmt::{self, Debug, Formatter},
    io as std_io,
    net::ToSocketAddrs,
    sync::{Arc, OnceLock},
};
use trillium_server_common::{Connector, QuicConnection, QuicConnector, RuntimeTrait};

/// Client-side QUIC configuration for HTTP/3, backed by quinn.
///
/// Has no type parameters — runtime and UDP types are inferred from the connector
/// passed to [`Client::new_with_quic`](trillium_client::Client::new_with_quic),
/// mirroring the server-side [`QuicConfig`](crate::QuicConfig) pattern.
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
    endpoint_v4: OnceLock<quinn::Endpoint>,
    endpoint_v6: OnceLock<quinn::Endpoint>,
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
            endpoint_v4: OnceLock::new(),
            endpoint_v6: OnceLock::new(),
        }
    }

    /// Return the shared endpoint for the given peer address family, creating it on first call.
    ///
    /// IPv4 and IPv6 peers each require a socket bound to the matching family, so two endpoints
    /// are maintained and selected here based on `addr`.
    fn endpoint<R: RuntimeTrait + Unpin, U: SocketTransport>(
        &self,
        addr: std::net::SocketAddr,
        runtime: &R,
    ) -> std_io::Result<&quinn::Endpoint> {
        let (lock, bind_addr) = if addr.is_ipv6() {
            (&self.endpoint_v6, "[::]:0")
        } else {
            (&self.endpoint_v4, "0.0.0.0:0")
        };
        if let Some(ep) = lock.get() {
            return Ok(ep);
        }
        let socket = std::net::UdpSocket::bind(bind_addr)?;
        let quinn_runtime = TrilliumRuntime::<R, U>::new(runtime.clone());
        let ep = quinn::Endpoint::new(
            quinn::EndpointConfig::default(),
            None, // client-only, no server config
            socket,
            quinn_runtime,
        )?;
        // If two threads race here, one loses; drop the duplicate endpoint.
        let _ = lock.set(ep);
        Ok(lock.get().unwrap())
    }
}

impl<C> QuicConnector<C> for ClientQuicConfig
where
    C: Connector,
    C::Runtime: Unpin,
    C::Udp: SocketTransport,
{
    async fn connect<'a>(
        &'a self,
        host: &'a str,
        port: u16,
        runtime: &'a C::Runtime,
    ) -> std_io::Result<QuicConnection> {
        // Blocking DNS resolution. Acceptable here because:
        // (a) connections are infrequent, (b) results are typically cached by the OS.
        // TODO: replace with async DNS when RuntimeTrait gains that capability.
        let addr = (host, port).to_socket_addrs()?.next().ok_or_else(|| {
            std_io::Error::new(
                std_io::ErrorKind::NotFound,
                "no addresses resolved for host",
            )
        })?;

        let endpoint = self.endpoint::<C::Runtime, C::Udp>(addr, runtime)?;

        let connection = endpoint
            .connect_with(self.client_config.clone(), addr, host)
            .map_err(|e| std_io::Error::new(std_io::ErrorKind::InvalidInput, e))?
            .await
            .map_err(|e| std_io::Error::new(std_io::ErrorKind::ConnectionRefused, e))?;

        let quic_conn = QuinnConnection::new(connection);
        setup_h3_streams(&quic_conn, runtime).await?;

        Ok(QuicConnection::from(quic_conn))
    }
}

/// Set up the three mandatory HTTP/3 unidirectional streams on a new client connection,
/// and spawn tasks to manage the server's inbound uni streams.
///
/// Per RFC 9114 §6.2, both endpoints must open a control stream and QPACK encoder/decoder
/// streams. The client opens them immediately after the QUIC handshake.
async fn setup_h3_streams<R: RuntimeTrait>(
    conn: &QuinnConnection,
    runtime: &R,
) -> std_io::Result<()> {
    use trillium_server_common::QuicConnectionTrait as _;
    // --- Outbound streams (client → server) ---

    let (_, mut control) = conn.open_uni().await?;
    let (_, mut qpack_enc) = conn.open_uni().await?;
    let (_, mut qpack_dec) = conn.open_uni().await?;

    // Control stream: stream type (0x00) + empty SETTINGS frame (type=0x04, length=0x00).
    // We advertise no settings for now; the static QPACK table requires nothing extra.
    control.write_all(&[0x00, 0x04, 0x00]).await?;
    control.flush().await?;

    // QPACK encoder/decoder streams: just the one-byte stream type identifier.
    qpack_enc.write_all(&[0x02]).await?;
    qpack_enc.flush().await?;
    qpack_dec.write_all(&[0x03]).await?;
    qpack_dec.flush().await?;

    // Hold all three streams open for the connection lifetime. Per RFC 9114 §6.2.1,
    // closing a control stream is a connection error.
    let quinn_conn = conn.inner().clone();
    runtime.spawn(async move {
        quinn_conn.closed().await;
        // Streams are dropped here, sending FIN after the connection is already closed.
        drop((control, qpack_enc, qpack_dec));
    });

    // --- Inbound streams (server → client) ---

    // Accept and drain the server's control, QPACK encoder, and QPACK decoder streams.
    // We discard the data for now (static table only, no SETTINGS needed for basic operation).
    // TODO: parse server SETTINGS (for MAX_FIELD_SECTION_SIZE) and handle GOAWAY.
    let conn_for_accept = conn.clone();
    let runtime_for_drain = runtime.clone();
    runtime.spawn(async move {
        while let Ok((_, mut recv)) = conn_for_accept.accept_uni().await {
            runtime_for_drain.spawn(async move {
                io::copy(&mut recv, &mut io::sink()).await.ok();
            });
        }
    });

    Ok(())
}
