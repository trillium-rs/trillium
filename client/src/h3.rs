use crate::{Pool, h3::alt_svc::DEFAULT_BROKEN_DURATION, pool::PoolEntry};
use alt_svc::AltSvcCache;
use std::{
    io::{self, ErrorKind},
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::Duration,
};
use trillium_http::{
    HttpContext,
    h3::{H3Connection, H3Error, H3ErrorCode, UniStreamResult},
};
use trillium_server_common::{
    ArcedConnector, ArcedQuicClientConfig, ArcedQuicEndpoint, Connector, QuicConnection, Runtime,
    url::{Origin, Url},
};

mod alt_svc;

/// Shared state for HTTP/3 support on a [`Client`].
///
/// Present only when the client has been configured with a [`QuicClientConfig`] via
/// [`Client::new_with_quic`]. All fields are cheaply cloneable (Arc-backed).
#[derive(Clone, Debug)]
pub(crate) struct H3ClientState {
    pub(crate) config: ArcedQuicClientConfig,
    pub(crate) pool: Pool<Origin, QuicConnection>,
    pub(crate) alt_svc: AltSvcCache,
    pub(crate) broken_duration: Duration,
    endpoint_v4: OnceLockEndpoint,
    endpoint_v6: OnceLockEndpoint,
}

/// A cloneable, lazily-initialized endpoint holder.
#[derive(Clone, Debug)]
struct OnceLockEndpoint(Arc<OnceLock<ArcedQuicEndpoint>>);

impl Default for OnceLockEndpoint {
    fn default() -> Self {
        Self(Arc::new(OnceLock::new()))
    }
}

impl H3ClientState {
    pub(crate) fn update_alt_svc(&self, alt_svc: &str, url: &Url) {
        self.alt_svc.update(alt_svc, url);
    }

    pub(crate) fn mark_broken(&self, origin: &Origin) {
        if let Some(mut entry) = self.alt_svc.get_mut(origin) {
            entry.mark_broken(self.broken_duration);
        }
    }

    /// Get a pooled QUIC connection for this origin, or establish a new one.
    ///
    /// When a new connection is established, spawns the mandatory HTTP/3 control and QPACK
    /// streams using `H3Connection` from trillium-http.
    pub(crate) async fn get_or_create_quic_conn(
        &self,
        origin: &Origin,
        host: &str,
        port: u16,
        connector: &ArcedConnector,
        context: &Arc<HttpContext>,
    ) -> io::Result<QuicConnection> {
        if let Some(conn) = self.pool.peek_candidate(origin) {
            return Ok(conn);
        }

        let addr = *connector
            .resolve(host, port)
            .await?
            .first()
            .ok_or_else(|| io::Error::new(ErrorKind::NotFound, "no addresses resolved for host"))?;

        let endpoint = self.endpoint_for(addr)?;
        let conn = endpoint.connect(addr, host).await?;

        setup_h3_connection(&conn, context, &connector.runtime());

        self.pool
            .insert(origin.clone(), PoolEntry::new(conn.clone(), None));
        Ok(conn)
    }

    /// Get or create the endpoint for the given peer address family.
    fn endpoint_for(&self, addr: SocketAddr) -> io::Result<ArcedQuicEndpoint> {
        let (holder, bind_addr) = if addr.is_ipv6() {
            (&self.endpoint_v6, "[::]:0")
        } else {
            (&self.endpoint_v4, "0.0.0.0:0")
        };

        if let Some(ep) = holder.0.get() {
            return Ok(ep.clone());
        }

        let bind_addr: SocketAddr = bind_addr
            .parse()
            .map_err(|e| io::Error::new(ErrorKind::InvalidInput, e))?;
        let ep = self.config.bind(bind_addr)?;

        // If two threads race, one loses; use whichever got there first.
        let _ = holder.0.set(ep);
        Ok(holder.0.get().unwrap().clone())
    }

    pub(crate) fn new(config: ArcedQuicClientConfig) -> Self {
        Self {
            config,
            pool: Default::default(),
            alt_svc: AltSvcCache::default(),
            broken_duration: DEFAULT_BROKEN_DURATION,
            endpoint_v4: OnceLockEndpoint::default(),
            endpoint_v6: OnceLockEndpoint::default(),
        }
    }
}

/// Spawn the mandatory HTTP/3 streams on a newly established QUIC connection.
///
/// This mirrors the server-side `run_h3_connection` in trillium-server-common,
/// using the same `H3Connection` from trillium-http for wire-protocol handling.
fn setup_h3_connection(quic_conn: &QuicConnection, context: &Arc<HttpContext>, runtime: &Runtime) {
    let h3 = H3Connection::new(context.clone());

    // Outbound control stream — sends SETTINGS, then GOAWAY on shutdown.
    spawn_outbound_control_stream(quic_conn, &h3, runtime);

    // Outbound QPACK encoder/decoder streams — held open for the connection lifetime.
    spawn_qpack_encoder_stream(quic_conn, &h3, runtime);
    spawn_qpack_decoder_stream(quic_conn, &h3, runtime);

    // Inbound uni streams — handles the server's control, QPACK, and unknown streams.
    spawn_inbound_uni_streams(quic_conn, &h3, runtime);

    // Reject inbound bidi streams — servers must not open client-initiated streams (RFC 9114 §6.1).
    spawn_reject_inbound_bidi_streams(quic_conn, runtime);
}

fn spawn_outbound_control_stream(conn: &QuicConnection, h3: &Arc<H3Connection>, runtime: &Runtime) {
    let (conn, h3) = (conn.clone(), h3.clone());
    runtime.spawn(async move {
        let _guard = h3.swansong().guard();
        let result: Result<(), H3Error> =
            async { h3.run_outbound_control(conn.open_uni().await?.1).await }.await;
        if let Err(error) = result {
            log::debug!("client H3 control stream error: {error}");
        }
    });
}

fn spawn_qpack_encoder_stream(conn: &QuicConnection, h3: &Arc<H3Connection>, runtime: &Runtime) {
    let (conn, h3) = (conn.clone(), h3.clone());
    runtime.spawn(async move {
        let result: Result<(), H3Error> =
            async { h3.run_encoder(conn.open_uni().await?.1).await }.await;
        if let Err(error) = result {
            log::debug!("client H3 qpack encoder error: {error}");
        }
    });
}

fn spawn_qpack_decoder_stream(conn: &QuicConnection, h3: &Arc<H3Connection>, runtime: &Runtime) {
    let (conn, h3) = (conn.clone(), h3.clone());
    runtime.spawn(async move {
        let result: Result<(), H3Error> =
            async { h3.run_decoder(conn.open_uni().await?.1).await }.await;
        if let Err(error) = result {
            log::debug!("client H3 qpack decoder error: {error}");
        }
    });
}

fn spawn_reject_inbound_bidi_streams(conn: &QuicConnection, runtime: &Runtime) {
    let conn = conn.clone();
    runtime.spawn(async move {
        while let Ok((_stream_id, mut transport)) = conn.accept_bidi().await {
            log::debug!("rejecting unexpected inbound bidi stream from server");
            let code = H3ErrorCode::StreamCreationError;
            transport.stop(code.into());
            transport.reset(code.into());
        }
    });
}

fn spawn_inbound_uni_streams(conn: &QuicConnection, h3: &Arc<H3Connection>, runtime: &Runtime) {
    let (conn, h3, runtime) = (conn.clone(), h3.clone(), runtime.clone());
    runtime.clone().spawn(async move {
        while let Ok((_stream_id, recv)) = conn.accept_uni().await {
            let (conn_for_error, h3) = (conn.clone(), h3.clone());
            runtime.spawn(async move {
                match h3.process_inbound_uni(recv).await {
                    Ok(UniStreamResult::Handled) => {}
                    Ok(
                        UniStreamResult::WebTransport { mut stream, .. }
                        | UniStreamResult::Unknown { mut stream, .. },
                    ) => {
                        // Client doesn't expect WebTransport or unknown uni streams from server.
                        log::debug!("ignoring unexpected inbound uni stream");
                        stream.stop(H3ErrorCode::StreamCreationError.into());
                    }
                    Err(error) => {
                        log::debug!("client H3 inbound uni stream error: {error}");
                        if let H3Error::Protocol(code) = error {
                            conn_for_error.close(code.into(), code.reason().as_bytes());
                        }
                        h3.shut_down().await;
                    }
                }
            });
        }
    });
}
