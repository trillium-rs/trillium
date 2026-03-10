use crate::stream::{Datagram, InboundBidiStream, InboundUniStream};
use async_channel::{Receiver, Sender};
use async_lock::Mutex;
use std::collections::HashMap;
use trillium_server_common::h3::web_transport::{WebTransportDispatch, WebTransportStream};

/// The concrete [`WebTransportDispatch`] implementation registered with the dispatcher.
///
/// Holds a routing channel and the shared session map. The dispatcher calls
/// [`dispatch`](WebTransportDispatch::dispatch) synchronously; streams are forwarded through
/// the channel to the routing task, which does the actual per-session delivery.
pub(crate) struct Router {
    routing_tx: Sender<WebTransportStream>,
    routing_rx: std::sync::Mutex<Option<Receiver<WebTransportStream>>>,
    pub(crate) sessions: Mutex<SessionRouter>,
}

impl Router {
    pub(crate) fn new(max_datagram_buffer: usize) -> Self {
        let (routing_tx, routing_rx) = async_channel::unbounded();
        Self {
            routing_tx,
            routing_rx: std::sync::Mutex::new(Some(routing_rx)),
            sessions: Mutex::new(SessionRouter::new(max_datagram_buffer)),
        }
    }

    /// Take the routing receiver. Returns `Some` exactly once — the caller
    /// spawns the routing task with it.
    pub(crate) fn take_routing_rx(&self) -> Option<Receiver<WebTransportStream>> {
        self.routing_rx.lock().unwrap().take()
    }
}

impl WebTransportDispatch for Router {
    fn dispatch(&self, stream: WebTransportStream) {
        self.routing_tx.try_send(stream).ok();
    }
}

struct SessionEntry {
    bidi_tx: Sender<InboundBidiStream>,
    uni_tx: Sender<InboundUniStream>,
    datagram_tx: Sender<Datagram>,
    datagram_rx: Receiver<Datagram>,
}

/// Routes inbound WebTransport streams to per-session channels.
///
/// Intended to live behind an `async_lock::Mutex`. The routing task and
/// upgrade handlers share access via `Arc<Mutex<SessionRouter>>`.
pub(crate) struct SessionRouter {
    sessions: HashMap<u64, SessionEntry>,
    pending: HashMap<u64, Vec<WebTransportStream>>,
    max_datagram_buffer: usize,
}

impl SessionRouter {
    pub(crate) fn new(max_datagram_buffer: usize) -> Self {
        Self {
            sessions: HashMap::new(),
            pending: HashMap::new(),
            max_datagram_buffer,
        }
    }

    /// Register a session, returning receivers for its bidi, uni, and datagram channels.
    ///
    /// Any streams that arrived before this session registered are drained
    /// into the channels before returning.
    pub(crate) fn register(
        &mut self,
        session_id: u64,
    ) -> (
        async_channel::Receiver<InboundBidiStream>,
        async_channel::Receiver<InboundUniStream>,
        async_channel::Receiver<Datagram>,
    ) {
        let (bidi_tx, bidi_rx) = async_channel::unbounded();
        let (uni_tx, uni_rx) = async_channel::unbounded();
        let (datagram_tx, datagram_rx) = async_channel::bounded(self.max_datagram_buffer);

        if let Some(buffered) = self.pending.remove(&session_id) {
            for stream in buffered {
                send_to_session(&bidi_tx, &uni_tx, stream);
            }
        }

        self.sessions.insert(
            session_id,
            SessionEntry {
                bidi_tx,
                uni_tx,
                datagram_tx,
                datagram_rx: datagram_rx.clone(),
            },
        );

        (bidi_rx, uni_rx, datagram_rx)
    }

    /// Remove a session from the router. Remaining senders are dropped,
    /// closing the channels.
    pub(crate) fn unregister(&mut self, session_id: u64) {
        self.sessions.remove(&session_id);
        self.pending.remove(&session_id);
    }

    /// Route an inbound datagram to its session.
    ///
    /// Parses the quarter-stream-ID prefix, looks up the session, and sends the
    /// payload. If the datagram buffer is full, the oldest datagram is dropped.
    /// Datagrams for unknown sessions are silently dropped.
    pub(crate) fn route_datagram(&mut self, data: &[u8]) {
        let Ok((quarter_id, consumed)) = trillium_http::h3::quic_varint::decode::<u64>(data) else {
            log::debug!("datagram with invalid quarter-stream-ID varint");
            return;
        };
        let session_id = quarter_id * 4;
        let payload = Datagram::from(data[consumed..].to_vec());

        if let Some(entry) = self.sessions.get(&session_id) {
            match entry.datagram_tx.try_send(payload) {
                Ok(()) => {}
                Err(async_channel::TrySendError::Full(payload)) => {
                    // Drop oldest, send newest
                    let _ = entry.datagram_rx.try_recv();
                    let _ = entry.datagram_tx.try_send(payload);
                }
                Err(async_channel::TrySendError::Closed(_)) => {
                    log::debug!("session {session_id} datagram channel closed");
                }
            }
        }
    }

    /// Route an inbound stream to its session, or buffer it if the session
    /// hasn't registered yet.
    pub(crate) fn route(&mut self, stream: WebTransportStream) {
        let session_id = stream.session_id();
        if let Some(entry) = self.sessions.get(&session_id) {
            send_to_session(&entry.bidi_tx, &entry.uni_tx, stream);
        } else {
            log::trace!("pending {stream:?}");
            self.pending.entry(session_id).or_default().push(stream);
        }
    }
}

fn send_to_session(
    bidi_tx: &Sender<InboundBidiStream>,
    uni_tx: &Sender<InboundUniStream>,
    stream: WebTransportStream,
) {
    log::trace!("routing {stream:?}");

    match stream {
        WebTransportStream::Bidi {
            session_id,
            stream: transport,
            buffer,
        } => {
            if bidi_tx
                .try_send(InboundBidiStream::new(transport, buffer))
                .is_err()
            {
                log::debug!("session {session_id} bidi channel closed, dropping stream");
            }
        }
        WebTransportStream::Uni {
            session_id,
            stream,
            buffer,
        } => {
            if uni_tx
                .try_send(InboundUniStream::new(stream, buffer))
                .is_err()
            {
                log::debug!("session {session_id} uni channel closed, dropping stream");
            }
        }
    }
}
