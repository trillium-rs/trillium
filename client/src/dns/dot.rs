//! DNS-over-TLS ([RFC 7858]): queries are pipelined over a persistent, shared TLS connection to
//! the resolver (default port 853), each length-prefixed with its 2-byte size (RFC 1035).
//!
//! The connection is established lazily and kept alive indefinitely, reconnecting on next use after
//! it closes. Because a single byte stream carries concurrent queries whose responses may arrive in
//! any order, each query stamps a unique DNS message ID and a per-connection driver task
//! demultiplexes responses back by that ID — unlike DoH and DoQ, which keep ID 0 because the HTTP
//! request / QUIC stream already correlates request to response.
//!
//! [RFC 7858]: https://www.rfc-editor.org/rfc/rfc7858

use super::framing::{frame, take_frame};
use crate::Client;
use async_channel::{Receiver, Sender, bounded, unbounded};
use async_lock::Mutex;
use futures_lite::{AsyncReadExt, AsyncWriteExt, future};
use std::{
    borrow::Cow,
    collections::HashMap,
    io::{self, ErrorKind},
    sync::{
        Arc,
        atomic::{AtomicU16, Ordering},
    },
};
use trillium_server_common::{Connector, Destination, Transport, url::Url};

/// DoT transport: the resolver endpoint URL (always `https`, default port 853), a per-resolver DNS
/// message-ID counter, and the cached driver handle to the live connection.
#[derive(Debug, Clone)]
pub(super) struct Dot {
    resolver: Url,
    /// Per-resolver, monotonically wrapping DNS message ID. Survives reconnects — IDs need only be
    /// unique among queries concurrently in flight, and 65536 wrapping values vastly exceed that.
    next_id: Arc<AtomicU16>,
    /// The driver's outbound queue, present once a connection has been established and still live.
    /// `None` before first use and after a connection dies; rebuilt under the lock on next use.
    conn: Arc<Mutex<Option<Sender<Outbound>>>>,
}

/// One query handed to the driver: the framed bytes to write, the ID to demultiplex its response
/// by, and the channel to deliver that response on.
#[derive(Debug)]
struct Outbound {
    id: u16,
    framed: Vec<u8>,
    resp: Sender<Vec<u8>>,
}

impl Dot {
    pub(super) fn new(mut resolver: Url) -> Self {
        // DoT lives on port 853 (RFC 7858), but the resolver URL carries the `https` scheme so
        // the TLS connector can dial it — and `https` defaults to 443. Pin 853 unless the URL named
        // a port explicitly, or the connector would silently dial the resolver's HTTPS port.
        if resolver.port().is_none() {
            let _ = resolver.set_port(Some(853));
        }
        Self {
            resolver,
            next_id: Arc::new(AtomicU16::new(0)),
            conn: Arc::new(Mutex::new(None)),
        }
    }

    pub(super) fn host(&self) -> Option<&str> {
        self.resolver.host_str()
    }

    pub(super) fn resolver(&self) -> &Url {
        &self.resolver
    }

    /// Pipeline `query` over the shared connection and await its response, reconnecting once if the
    /// cached connection has died.
    pub(super) async fn exchange(&self, client: &Client, query: Vec<u8>) -> io::Result<Vec<u8>> {
        log::trace!(
            "DoT exchange to {}: {}-byte query",
            self.resolver,
            query.len()
        );
        let tx = self.connection(client).await?;
        match self.send(&tx, query.clone()).await {
            Ok(response) => {
                log::trace!(
                    "DoT exchange to {}: {}-byte response",
                    self.resolver,
                    response.len()
                );
                Ok(response)
            }
            Err(_) => {
                // The driver is gone (connection closed or errored). Drop it and re-establish once;
                // a failure on the fresh connection then propagates.
                log::debug!("DoT connection to {} unusable; reconnecting", self.resolver);
                self.invalidate(&tx).await;
                let tx = self.connection(client).await?;
                self.send(&tx, query).await
            }
        }
    }

    /// Stamp a fresh ID into the query, hand it to the driver, and await the matching response. An
    /// error means the driver is gone — either its outbound queue is closed (`send`) or it dropped
    /// our response channel on teardown (`recv`).
    async fn send(&self, tx: &Sender<Outbound>, mut query: Vec<u8>) -> io::Result<Vec<u8>> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        // `build_query` always sets the message ID to 0; the pipelined path overwrites it here, per
        // connection, after the shared codec — the ID is a DoT framing concern, not a query one.
        if query.len() < 2 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "DNS query too short",
            ));
        }
        query[0..2].copy_from_slice(&id.to_be_bytes());

        log::trace!("DoT query id {id} queued for {}", self.resolver);
        let (resp, resp_rx) = bounded(1);
        let framed = frame(&query)?;
        tx.send(Outbound { id, framed, resp })
            .await
            .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "DoT driver stopped"))?;
        resp_rx
            .recv()
            .await
            .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "DoT connection closed"))
    }

    /// The outbound queue of the live driver, establishing a connection (and spawning its driver)
    /// if absent. Holding the lock across the connect single-flights concurrent queries onto
    /// one connection.
    async fn connection(&self, client: &Client) -> io::Result<Sender<Outbound>> {
        let mut guard = self.conn.lock().await;
        if let Some(tx) = guard.as_ref() {
            return Ok(tx.clone());
        }
        let tx = self.connect(client).await?;
        *guard = Some(tx.clone());
        Ok(tx)
    }

    /// Clear the cached connection, but only if it is still the one `stale` belongs to — a
    /// concurrent query may already have reconnected, and clearing that fresh connection would
    /// needlessly tear it down.
    async fn invalidate(&self, stale: &Sender<Outbound>) {
        let mut guard = self.conn.lock().await;
        if guard.as_ref().is_some_and(|tx| tx.same_channel(stale)) {
            *guard = None;
        }
    }

    /// Establish the TLS connection to the resolver, spawn its driver task, and return the outbound
    /// queue. The driver owns the transport for the connection's lifetime.
    async fn connect(&self, client: &Client) -> io::Result<Sender<Outbound>> {
        log::debug!("DoT connecting to {} (alpn dot)", self.resolver);
        let destination =
            Destination::from_url(&self.resolver)?.with_alpn([Cow::Borrowed(&b"dot"[..])]);
        let transport = client.connector().connect_to(destination).await?;
        log::debug!(
            "DoT connected to {}; negotiated alpn {:?}",
            self.resolver,
            transport
                .negotiated_alpn()
                .map(|a| String::from_utf8_lossy(&a).into_owned())
        );

        let (tx, rx) = unbounded();
        // The handle is dropped deliberately: spawned tasks detach on drop and run to completion,
        // here until the connection ends or every `Dot` handle is gone (the outbound queue closes).
        client.connector().runtime().spawn(drive(transport, rx));
        Ok(tx)
    }
}

/// Drive one DoT connection: write each queued query and read responses off the shared stream,
/// routing each back to its waiting query by DNS message ID. Runs until every [`Dot`] handle is
/// dropped (the outbound queue closes) or the connection ends, at which point dropping the inflight
/// map fails every still-waiting query so it can reconnect.
async fn drive(mut transport: Box<dyn Transport>, requests: Receiver<Outbound>) {
    log::trace!("DoT driver started");
    let mut inflight: HashMap<u16, Sender<Vec<u8>>> = HashMap::new();
    let mut read_buf = Vec::new();
    let mut chunk = [0u8; 2048];

    let reason = loop {
        // Outbound first so a steady response stream can't starve query submission. `future::or`
        // drops the losing branch each iteration: a dropped `recv` loses no message, and a dropped
        // single-`poll_read` consumes nothing — both cancel-safe. The write awaits to completion
        // inside its arm, briefly pausing reads, which is negligible for DNS-sized messages.
        let event = future::or(
            async { Event::Outbound(requests.recv().await.ok()) },
            async { Event::Read(transport.read(&mut chunk).await) },
        )
        .await;

        match event {
            // All `Dot` handles dropped — nothing more will ever be sent.
            Event::Outbound(None) => break "all handles dropped",
            Event::Outbound(Some(outbound)) => {
                log::trace!("DoT driver writing query id {}", outbound.id);
                inflight.insert(outbound.id, outbound.resp);
                if let Err(e) = transport.write_all(&outbound.framed).await {
                    log::debug!("DoT driver write failed: {e}");
                    break "write error";
                }
            }
            Event::Read(Ok(0)) => break "connection closed by resolver",
            Event::Read(Err(e)) => {
                log::debug!("DoT driver read failed: {e}");
                break "read error";
            }
            Event::Read(Ok(n)) => {
                log::trace!("DoT driver read {n} bytes");
                read_buf.extend_from_slice(&chunk[..n]);
                while let Some(message) = take_frame(&mut read_buf) {
                    // The first two bytes of a DNS message are its ID — the one we stamped on send.
                    let [hi, lo, ..] = message[..] else { continue };
                    let id = u16::from_be_bytes([hi, lo]);
                    if let Some(resp) = inflight.remove(&id) {
                        log::trace!(
                            "DoT driver routing {}-byte response to id {id}",
                            message.len()
                        );
                        let _ = resp.try_send(message);
                    } else {
                        log::trace!("DoT driver got response for unknown id {id}; dropping");
                    }
                }
            }
        }
    };

    log::trace!(
        "DoT driver ending ({reason}); {} queries still in flight",
        inflight.len()
    );
}

/// The two things the driver loop races on: a query to send, or bytes to read.
enum Event {
    Outbound(Option<Outbound>),
    Read(io::Result<usize>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use trillium_testing::{TestTransport, harness, test};

    /// A response message whose first two bytes are its DNS ID (what the driver routes on),
    /// followed by a distinguishing tag so we can confirm each query got *its* answer.
    fn message(id: u16, tag: u8) -> Vec<u8> {
        let mut message = id.to_be_bytes().to_vec();
        message.extend_from_slice(&[tag; 4]);
        message
    }

    #[test(harness)]
    async fn driver_demultiplexes_out_of_order() {
        let (client_side, mut server_side) = TestTransport::new();
        let (tx, rx) = unbounded::<Outbound>();

        // Resolver half: read both framed queries, then reply in *reverse* ID order to exercise the
        // demux — a correlation-by-arrival-order driver would mis-route these.
        let resolver = async move {
            for _ in 0..2 {
                let mut len = [0u8; 2];
                server_side.read_exact(&mut len).await.unwrap();
                let mut query = vec![0u8; usize::from(u16::from_be_bytes(len))];
                server_side.read_exact(&mut query).await.unwrap();
            }
            server_side.write_all(&frame(&message(2, 0x22)).unwrap());
            server_side.write_all(&frame(&message(1, 0x11)).unwrap());
        };

        let exchange = async move {
            let (resp1, rx1) = bounded(1);
            let (resp2, rx2) = bounded(1);
            tx.send(Outbound {
                id: 1,
                framed: frame(&message(1, 0)).unwrap(),
                resp: resp1,
            })
            .await
            .unwrap();
            tx.send(Outbound {
                id: 2,
                framed: frame(&message(2, 0)).unwrap(),
                resp: resp2,
            })
            .await
            .unwrap();
            // Each query gets the response carrying its own ID, regardless of arrival order.
            assert_eq!(rx1.recv().await.unwrap(), message(1, 0x11));
            assert_eq!(rx2.recv().await.unwrap(), message(2, 0x22));
            // Dropping `tx` closes the outbound queue, ending the driver.
        };

        future::zip(
            future::zip(resolver, exchange),
            drive(Box::new(client_side), rx),
        )
        .await;
    }
}
