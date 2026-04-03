//! WebTransport support for Trillium.
//!
//! This crate provides a [`WebTransport`] handler that accepts WebTransport sessions over
//! HTTP/3, and a [`WebTransportConnection`] handle for sending and receiving streams and
//! datagrams within each session.
//!
//! WebTransport requires an HTTP/3-capable server adapter configured with a QUIC endpoint
//! and TLS.

#[cfg(test)]
#[doc = include_str!("../README.md")]
mod readme {}

mod session_router;
mod stream;

use crate::session_router::Router;
pub use crate::stream::{
    Datagram, InboundBidiStream, InboundStream, InboundUniStream, OutboundBidiStream,
    OutboundUniStream,
};
use async_channel::Receiver;
use futures_lite::AsyncWriteExt;
use std::{
    io,
    sync::{Arc, OnceLock},
};
use swansong::Swansong;
use trillium::{Conn, Handler, Info, Method, Status, Transport, Upgrade};
use trillium_http::h3::{H3Connection, quic_varint};
use trillium_server_common::{
    QuicConnection, Runtime,
    h3::{
        StreamId,
        web_transport::{WebTransportDispatcher, WebTransportStream},
    },
};

/// A handle to an active WebTransport session.
///
/// Passed to your [`WebTransportHandler`] when a client opens a WebTransport session.
/// Use it to accept streams from the client, open server-initiated streams, and exchange
/// datagrams.
pub struct WebTransportConnection {
    session_id: u64,
    bidi_rx: Receiver<InboundBidiStream>,
    uni_rx: Receiver<InboundUniStream>,
    datagram_rx: Receiver<Datagram>,
    swansong: Swansong,
    upgrade: Upgrade,
    h3_connection: Arc<H3Connection>,
    quic_connection: QuicConnection,
    runtime: Runtime,
}

impl WebTransportConnection {
    /// Accept the next inbound bidirectional stream for this session.
    ///
    /// Returns `None` when the session is shutting down or the QUIC connection has closed.
    pub async fn accept_bidi(&self) -> Option<InboundBidiStream> {
        self.swansong.interrupt(self.bidi_rx.recv()).await?.ok()
    }

    /// Returns the async runtime for this server.
    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// Returns the underlying HTTP/3 connection.
    pub fn h3_connection(&self) -> &H3Connection {
        &self.h3_connection
    }

    /// Returns the HTTP CONNECT upgrade that initiated this WebTransport session.
    ///
    /// Provides access to request headers, connection state, and peer information from
    /// the CONNECT request.
    pub fn upgrade(&self) -> &Upgrade {
        &self.upgrade
    }

    /// Returns a mutable reference to the HTTP CONNECT upgrade that initiated this session.
    pub fn upgrade_mut(&mut self) -> &mut Upgrade {
        &mut self.upgrade
    }

    /// Accept the next inbound unidirectional stream for this session.
    ///
    /// Returns `None` when the session is shutting down or the QUIC connection has closed.
    pub async fn accept_uni(&self) -> Option<InboundUniStream> {
        self.swansong.interrupt(self.uni_rx.recv()).await?.ok()
    }

    /// Receive the next datagram for this session.
    ///
    /// Returns `None` when the session is shutting down or the QUIC connection has closed.
    pub async fn recv_datagram(&self) -> Option<Datagram> {
        self.swansong.interrupt(self.datagram_rx.recv()).await?.ok()
    }

    /// Accept the next inbound stream for this session.
    ///
    /// Races the bidi and uni stream channels and returns whichever arrives first.
    /// Returns `None` when the session ends.
    ///
    /// Datagrams are intentionally excluded — use [`recv_datagram`](Self::recv_datagram)
    /// in a separate concurrent loop, as datagrams typically require lower latency
    /// than stream acceptance.
    pub async fn accept_next_stream(&self) -> Option<InboundStream> {
        futures_lite::future::race(
            async { self.accept_bidi().await.map(InboundStream::Bidi) },
            async { self.accept_uni().await.map(InboundStream::Uni) },
        )
        .await
    }

    /// Send an unreliable datagram to the client.
    ///
    /// Returns an error if the QUIC connection does not support datagrams or the payload is
    /// too large.
    pub fn send_datagram(&self, payload: &[u8]) -> io::Result<()> {
        let quarter_id = self.session_id / 4;
        let header_len = quic_varint::encoded_len(quarter_id);
        let mut buf = vec![0u8; header_len + payload.len()];
        quic_varint::encode(quarter_id, &mut buf).unwrap();
        buf[header_len..].copy_from_slice(payload);
        self.quic_connection.send_datagram(&buf)
    }

    /// Open a new server-initiated bidirectional stream for this session.
    pub async fn open_bidi(&self) -> io::Result<OutboundBidiStream> {
        let (_stream_id, mut transport) = self.quic_connection.open_bidi().await?;
        transport
            .write_all(&wt_bidi_header(self.session_id))
            .await?;
        Ok(OutboundBidiStream::new(transport))
    }

    /// Open a new server-initiated unidirectional stream for this session.
    pub async fn open_uni(&self) -> io::Result<OutboundUniStream> {
        let (_stream_id, mut stream) = self.quic_connection.open_uni().await?;
        stream.write_all(&wt_uni_header(self.session_id)).await?;
        Ok(OutboundUniStream::new(stream))
    }
}

enum RoutingAction {
    Stream(WebTransportStream),
    Datagram(Vec<u8>),
}

/// Encode the bidi stream header: signal value 0x41 + session_id.
fn wt_bidi_header(session_id: u64) -> Vec<u8> {
    let mut buf =
        vec![0u8; quic_varint::encoded_len(0x41u64) + quic_varint::encoded_len(session_id)];
    let mut offset = quic_varint::encode(0x41u64, &mut buf).unwrap();
    offset += quic_varint::encode(session_id, &mut buf[offset..]).unwrap();
    buf.truncate(offset);
    buf
}

/// Encode the uni stream header: stream type 0x54 + session_id.
fn wt_uni_header(session_id: u64) -> Vec<u8> {
    let mut buf =
        vec![0u8; quic_varint::encoded_len(0x54u64) + quic_varint::encoded_len(session_id)];
    let mut offset = quic_varint::encode(0x54u64, &mut buf).unwrap();
    offset += quic_varint::encode(session_id, &mut buf[offset..]).unwrap();
    buf.truncate(offset);
    buf
}

const DEFAULT_MAX_DATAGRAM_BUFFER: usize = 16;

/// A Trillium [`Handler`] that accepts WebTransport sessions.
///
/// Add this to your handler chain and provide a [`WebTransportHandler`] (or a closure) to
/// process each session.
///
/// # Example
///
/// ```no_run
/// use trillium_webtransport::{WebTransport, WebTransportConnection};
///
/// let handler = WebTransport::new(|conn: WebTransportConnection| async move {
///     while let Some(stream) = conn.accept_next_stream().await {
///         // handle stream...
/// # drop(stream);
///     }
/// });
/// ```
pub struct WebTransport<H> {
    runtime: OnceLock<Runtime>,
    max_datagram_buffer: usize,
    handler: H,
}

/// A handler for WebTransport sessions.
///
/// Any `Fn(WebTransportConnection) -> impl Future<Output = ()>` automatically implements this
/// trait, so you can pass a closure or async function directly to [`WebTransport::new`].
pub trait WebTransportHandler: Send + Sync + 'static {
    /// Handle a WebTransport session. Called once per client-initiated session.
    fn run(
        &self,
        web_transport_connection: WebTransportConnection,
    ) -> impl Future<Output = ()> + Send;
}

impl<Fun, Fut> WebTransportHandler for Fun
where
    Fun: Fn(WebTransportConnection) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send,
{
    async fn run(&self, web_transport_connection: WebTransportConnection) {
        self(web_transport_connection).await
    }
}

impl<H> WebTransport<H>
where
    H: WebTransportHandler,
{
    /// Create a new `WebTransport` handler that passes each session to `handler`.
    pub fn new(handler: H) -> Self {
        Self {
            handler,
            runtime: Default::default(),
            max_datagram_buffer: DEFAULT_MAX_DATAGRAM_BUFFER,
        }
    }

    /// Set the maximum number of datagrams to buffer per session.
    ///
    /// When the buffer is full, the oldest datagram is dropped to make room for the newest.
    ///
    /// - **`max > 1`** — FIFO ring-buffer that tolerates bursts up to `max` datagrams before
    ///   dropping. Good for ordered event streams where some loss is acceptable.
    /// - **`max = 1`** — "latest-only" semantics: if multiple datagrams arrive while your
    ///   [`recv_datagram`](WebTransportConnection::recv_datagram) loop is busy, only the most
    ///   recent is retained. Good for streaming state (positions, sensor readings) where older
    ///   values are invalidated by newer ones.
    ///
    /// Default: 16.
    pub fn with_max_datagram_buffer(mut self, max: usize) -> Self {
        self.max_datagram_buffer = max;
        self
    }

    fn runtime(&self) -> &Runtime {
        self.runtime.get().unwrap()
    }
}

struct WTUpgrade;

impl<H> Handler for WebTransport<H>
where
    H: WebTransportHandler,
{
    async fn run(&self, conn: Conn) -> Conn {
        let inner: &trillium_http::Conn<Box<dyn Transport>> = conn.as_ref();
        if inner.state().contains::<QuicConnection>() && conn.method() == Method::Connect
        // todo(jbr): try to figure out why chrome isn't sending a protocol
        //            && inner.protocol() == Some("webtransport-h3")
        //            && inner.authority().is_some(/*and something else?*/)
        {
            conn.with_state(WTUpgrade).with_status(Status::Ok).halt()
        } else {
            conn
        }
    }

    async fn init(&mut self, info: &mut Info) {
        self.runtime.get_or_init(|| {
            info.state::<Runtime>()
                .cloned()
                .expect("webtransport requires a Runtime")
        });

        info.http_config_mut()
            .set_h3_datagrams_enabled(true)
            .set_webtransport_enabled(true);
    }

    fn has_upgrade(&self, upgrade: &Upgrade) -> bool {
        upgrade.state().get::<WTUpgrade>().is_some()
    }

    async fn upgrade(&self, mut upgrade: Upgrade) {
        let Some(h3_connection) = upgrade.h3_connection() else {
            log::error!("missing H3Connection in upgrade state");
            return;
        };
        let Some(quic_connection) = upgrade.state_mut().take::<QuicConnection>() else {
            log::error!("missing QuicConnection in upgrade state");
            return;
        };
        let Some(stream_id) = upgrade.state_mut().take::<StreamId>() else {
            log::error!("missing StreamId in upgrade state");
            return;
        };
        let Some(dispatcher) = upgrade.state().get::<WebTransportDispatcher>().cloned() else {
            log::error!("missing WebTransportDispatcher in upgrade state");
            return;
        };

        let max_datagram_buffer = self.max_datagram_buffer;
        let Some(router) = dispatcher.get_or_init_with(|| Router::new(max_datagram_buffer)) else {
            log::error!("WebTransportDispatcher has a handler of an unexpected type");
            return;
        };

        // Spawn the routing task if we're the first session on this connection.
        if let Some(routing_rx) = router.take_routing_rx() {
            let router = router.clone();
            let quic = quic_connection.clone();
            self.runtime().clone().spawn(async move {
                loop {
                    let action = futures_lite::future::race(
                        async { routing_rx.recv().await.ok().map(RoutingAction::Stream) },
                        async {
                            let mut data = Vec::new();
                            quic.recv_datagram(|d| data.extend_from_slice(d))
                                .await
                                .ok()
                                .map(|()| RoutingAction::Datagram(data))
                        },
                    )
                    .await;
                    match action {
                        Some(RoutingAction::Stream(stream)) => {
                            router.sessions.lock().await.route(stream);
                        }
                        Some(RoutingAction::Datagram(data)) => {
                            router.sessions.lock().await.route_datagram(&data);
                        }
                        None => break,
                    }
                }
            });
        }

        let session_id = stream_id.into();
        log::trace!("starting webtransport session {session_id}");
        let session_swansong = h3_connection.swansong().child();
        let (bidi_rx, uni_rx, datagram_rx) = router.sessions.lock().await.register(session_id);

        let runtime = self.runtime().clone();

        self.handler
            .run(WebTransportConnection {
                session_id,
                bidi_rx,
                uni_rx,
                datagram_rx,
                swansong: session_swansong.clone(),
                upgrade,
                h3_connection,
                quic_connection,
                runtime,
            })
            .await;

        log::trace!("finished handler, cleaning up");

        session_swansong.shut_down().await;
        router.sessions.lock().await.unregister(session_id);
    }
}
