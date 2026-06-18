use crate::{
    Pool,
    h3::alt_svc::DEFAULT_BROKEN_DURATION,
    pool::{Acquire, PoolEntryStatus},
};
use alt_svc::AltSvcCache;
use std::{
    io::{self, ErrorKind},
    net::SocketAddr,
    sync::{Arc, OnceLock},
    time::Duration,
};
use trillium_http::{
    HttpContext,
    h3::{H3Connection, H3Error, H3ErrorCode, H3StreamResult, UniStreamResult},
};
#[cfg(feature = "webtransport")]
use trillium_server_common::h3::web_transport::{WebTransportDispatcher, WebTransportStream};
use trillium_server_common::{
    ArcedConnector, ArcedQuicClientConfig, ArcedQuicEndpoint, Connector, QuicConnection, Runtime,
    url::{Origin, Url},
};

mod alt_svc;

/// A pooled HTTP/3 connection: the QUIC connection, the H3 wrapper, and (when the
/// `webtransport` feature is enabled) a slot for a lazily-created
/// [`WebTransportDispatcher`]. The slot stays empty until the first `webtransport_connect`
/// on this origin; until then, server-initiated WT streams are rejected outright. Cheaply
/// cloneable.
#[derive(Clone, Debug)]
pub(crate) struct H3PoolEntry {
    pub(crate) quic_conn: QuicConnection,
    pub(crate) h3: Arc<H3Connection>,
    #[cfg(feature = "webtransport")]
    pub(crate) dispatcher: Arc<OnceLock<WebTransportDispatcher>>,
}

impl H3PoolEntry {
    /// How the pool should treat this entry: [`Dead`] once the H3 connection's swansong has left
    /// the running state (shutting down or closed), otherwise [`Available`]. QUIC connections
    /// multiplex, so a running connection is always reusable. This is the swansong-level signal;
    /// a connection closed purely at the QUIC layer is caught on the next `open_bidi` instead.
    ///
    /// [`Dead`]: PoolEntryStatus::Dead
    /// [`Available`]: PoolEntryStatus::Available
    pub(crate) fn classify(&self) -> PoolEntryStatus {
        if self.h3.swansong().state().is_running() {
            PoolEntryStatus::Available
        } else {
            PoolEntryStatus::Dead
        }
    }
}

/// Shared state for HTTP/3 support on a [`Client`].
///
/// Present only when the client has been configured with a [`QuicClientConfig`] via
/// [`Client::new_with_quic`]. All fields are cheaply cloneable (Arc-backed).
#[derive(Clone, Debug)]
pub(crate) struct H3ClientState {
    pub(crate) config: ArcedQuicClientConfig,
    pub(crate) pool: Pool<Origin, H3PoolEntry>,
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
    /// streams.
    pub(crate) async fn get_or_create_quic_conn(
        &self,
        origin: &Origin,
        host: &str,
        port: u16,
        addrs: &[SocketAddr],
        connector: &ArcedConnector,
        context: &Arc<HttpContext>,
    ) -> io::Result<H3PoolEntry> {
        // QUIC connections are always multiplexed, so concurrent callers to a cold origin should
        // open one connection, not race N handshakes (each of which then stalls ~30s before
        // falling back). `acquire` elects one primer; the rest await its result.
        loop {
            match self.pool.acquire(origin.clone(), |entry| entry.classify()) {
                Acquire::Ready(entry) => return Ok(entry),
                Acquire::Await(cell) => match cell.wait().await {
                    Some(entry) => return Ok(entry.clone()),
                    // The primer's connect failed; take another lap to retry or re-await.
                    None => continue,
                },
                Acquire::Primer(guard) => {
                    // On error `?` returns and `guard` drops, waking waiters to retry; on success
                    // `resolve_ready` pools the connection and hands waiters a clone.
                    let entry = self
                        .connect_quic(host, port, addrs, connector, context)
                        .await?;
                    guard.resolve_ready(entry.clone(), None);
                    return Ok(entry);
                }
            }
        }
    }

    /// Establish a fresh QUIC connection to `host:port` and spawn its mandatory HTTP/3 streams.
    async fn connect_quic(
        &self,
        host: &str,
        port: u16,
        addrs: &[SocketAddr],
        connector: &ArcedConnector,
        context: &Arc<HttpContext>,
    ) -> io::Result<H3PoolEntry> {
        // Pre-resolved addresses (e.g. from DoH) take precedence; otherwise fall
        // back to the connector's own resolver.
        let addr = match addrs.first() {
            Some(addr) => *addr,
            None => *connector
                .resolve(host, port)
                .await?
                .first()
                .ok_or_else(|| {
                    io::Error::new(ErrorKind::NotFound, "no addresses resolved for host")
                })?,
        };

        let endpoint = self.endpoint_for(addr)?;
        let conn = endpoint.connect(addr, host).await?;

        Ok(setup_h3_connection(conn, context, &connector.runtime()))
    }

    /// Open a bare QUIC connection to `host` at `addr` advertising `alpn`, reusing this state's UDP
    /// endpoint but skipping all HTTP/3 setup (no control or QPACK streams). Used by DNS-over-QUIC,
    /// which drives raw bidi streams itself.
    #[cfg(feature = "hickory")]
    pub(crate) async fn connect_with_alpn(
        &self,
        host: &str,
        addr: SocketAddr,
        alpn: &[std::borrow::Cow<'static, [u8]>],
    ) -> io::Result<QuicConnection> {
        self.endpoint_for(addr)?
            .connect_with_alpn(addr, host, alpn)
            .await
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
fn setup_h3_connection(
    quic_conn: QuicConnection,
    context: &Arc<HttpContext>,
    runtime: &Runtime,
) -> H3PoolEntry {
    let h3 = H3Connection::new(context.clone());
    #[cfg(feature = "webtransport")]
    let dispatcher = Arc::new(OnceLock::new());

    spawn_outbound_control_stream(&quic_conn, &h3, runtime);
    spawn_qpack_encoder_stream(&quic_conn, &h3, runtime);
    spawn_qpack_decoder_stream(&quic_conn, &h3, runtime);

    spawn_inbound_uni_streams(
        &quic_conn,
        &h3,
        runtime,
        #[cfg(feature = "webtransport")]
        &dispatcher,
    );

    // Inbound bidi streams are only valid for WebTransport; RFC 9114 forbids
    // server-initiated request bidi.
    spawn_inbound_bidi_streams(
        &quic_conn,
        &h3,
        runtime,
        #[cfg(feature = "webtransport")]
        &dispatcher,
    );

    H3PoolEntry {
        quic_conn,
        h3,
        #[cfg(feature = "webtransport")]
        dispatcher,
    }
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

fn spawn_inbound_bidi_streams(
    conn: &QuicConnection,
    h3: &Arc<H3Connection>,
    runtime: &Runtime,
    #[cfg(feature = "webtransport")] dispatcher: &Arc<OnceLock<WebTransportDispatcher>>,
) {
    let (conn, h3, runtime) = (conn.clone(), h3.clone(), runtime.clone());
    #[cfg(feature = "webtransport")]
    let dispatcher = dispatcher.clone();

    runtime.clone().spawn(async move {
        while let Ok((stream_id, transport)) = conn.accept_bidi().await {
            #[cfg(feature = "webtransport")]
            let dispatcher = dispatcher.clone();
            let h3 = h3.clone();

            runtime.spawn(async move {
                let result = h3
                    .clone()
                    .process_inbound_bidi(transport, |conn| async move { conn }, stream_id)
                    .with_reset(|t, code| {
                        let raw = u64::from(code);
                        t.stop(raw);
                        t.reset(raw);
                    })
                    .await;

                match result {
                    Ok(H3StreamResult::WebTransport {
                        session_id,
                        mut transport,
                        buffer,
                    }) => {
                        #[cfg(feature = "webtransport")]
                        if let Some(dispatcher) = dispatcher.get() {
                            dispatcher.dispatch(WebTransportStream::Bidi {
                                session_id,
                                stream: transport,
                                buffer: buffer.into(),
                            });
                            return;
                        }
                        let _ = (session_id, &buffer);
                        log::debug!(
                            "inbound WT bidi stream before any WT session opened on this \
                             connection, rejecting"
                        );
                        transport.stop(H3ErrorCode::StreamCreationError.into());
                        transport.reset(H3ErrorCode::StreamCreationError.into());
                    }
                    Ok(H3StreamResult::Request(_)) => {
                        // process_inbound_bidi already sent a default response via our identity
                        // handler; just log the spec violation.
                        log::warn!(
                            "server opened a request bidi stream to client (RFC 9114 violation)"
                        );
                    }
                    Err(error) => {
                        log::debug!("client H3 inbound bidi stream error: {error}");
                    }
                }
            });
        }
    });
}

fn spawn_inbound_uni_streams(
    conn: &QuicConnection,
    h3: &Arc<H3Connection>,
    runtime: &Runtime,
    #[cfg(feature = "webtransport")] dispatcher: &Arc<OnceLock<WebTransportDispatcher>>,
) {
    let (conn, h3, runtime) = (conn.clone(), h3.clone(), runtime.clone());
    #[cfg(feature = "webtransport")]
    let dispatcher = dispatcher.clone();

    runtime.clone().spawn(async move {
        while let Ok((_stream_id, recv)) = conn.accept_uni().await {
            let (conn_for_error, h3) = (conn.clone(), h3.clone());
            #[cfg(feature = "webtransport")]
            let dispatcher = dispatcher.clone();

            runtime.spawn(async move {
                // Connection-level protocol errors must close the QUIC connection
                // before the recv stream drops; otherwise quinn's RecvStream::drop
                // sends STOP_SENDING and the peer's RESET_STREAM response can race
                // ahead, replacing our app error code with a transport-level code on
                // the wire. The closure runs inside process_inbound_uni_with_close
                // while the stream is still alive.
                let close_connection = {
                    let conn_for_close = conn_for_error.clone();
                    move |code: H3ErrorCode| {
                        conn_for_close.close(code.into(), code.reason().as_bytes());
                    }
                };
                match h3
                    .process_inbound_uni_with_close(recv, close_connection)
                    .await
                {
                    Ok(UniStreamResult::Handled) => {}

                    Ok(UniStreamResult::WebTransport {
                        session_id,
                        mut stream,
                        buffer,
                    }) => {
                        #[cfg(feature = "webtransport")]
                        if let Some(dispatcher) = dispatcher.get() {
                            dispatcher.dispatch(WebTransportStream::Uni {
                                session_id,
                                stream,
                                buffer: buffer.into(),
                            });
                            return;
                        }
                        let _ = (session_id, &buffer);
                        log::debug!(
                            "inbound WT uni stream before any WT session opened on this \
                             connection, rejecting"
                        );
                        stream.stop(H3ErrorCode::StreamCreationError.into());
                    }

                    Ok(UniStreamResult::Unknown { mut stream, .. }) => {
                        log::debug!("ignoring unknown inbound uni stream");
                        stream.stop(H3ErrorCode::StreamCreationError.into());
                    }
                    Err(error) => {
                        log::debug!("client H3 inbound uni stream error: {error}");
                        // Connection-level errors closed via the callback above
                        // (Connection::close is idempotent); this branch covers I/O
                        // errors plus stream-level errors that don't warrant close.
                        if let H3Error::Protocol(code) = error
                            && code.is_connection_error()
                        {
                            conn_for_error.close(code.into(), code.reason().as_bytes());
                        }
                        h3.shut_down().await;
                    }
                }
            });
        }
    });
}
