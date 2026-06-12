//! DNS-over-QUIC ([RFC 9250]): each query owns a fresh bidirectional stream on a QUIC connection
//! to the resolver (default port 853, ALPN `doq`), length-prefixed per RFC 1035 with a STREAM FIN
//! after the query.
//!
//! The QUIC connection is established over the client's HTTP/3 UDP endpoint but skips all HTTP/3
//! machinery — `doq` is its own ALPN, so there are no control or QPACK streams. The connection is
//! multiplexed and cached: a burst of A/AAAA/HTTPS queries shares one connection, each on its own
//! stream.
//!
//! [RFC 9250]: https://www.rfc-editor.org/rfc/rfc9250

use super::framing::length_prefixed_exchange;
use crate::Client;
use async_lock::Mutex;
use std::{
    borrow::Cow,
    io::{self, ErrorKind},
    sync::Arc,
};
use trillium_server_common::{Connector, QuicConnection, url::Url};

/// DoQ transport: the resolver endpoint URL (default port 853) plus the cached multiplexed QUIC
/// connection to it.
#[derive(Debug, Clone)]
pub(super) struct Doq {
    resolver: Url,
    /// The live QUIC connection to the resolver, shared across clones (Arc-backed) and lazily
    /// established. Wrapped in an async mutex so concurrent queries single-flight the connect and
    /// then multiplex over the one connection.
    connection: Arc<Mutex<Option<QuicConnection>>>,
}

impl Doq {
    pub(super) fn new(mut resolver: Url) -> Self {
        // DoQ lives on port 853 (RFC 9250). The resolver URL carries the `https` scheme for
        // uniformity with the other transports, and `https` defaults to 443; pin 853 unless a port
        // was named explicitly so the URL displayed in logs matches the port actually dialed.
        if resolver.port().is_none() {
            let _ = resolver.set_port(Some(853));
        }
        Self {
            resolver,
            connection: Arc::new(Mutex::new(None)),
        }
    }

    pub(super) fn host(&self) -> Option<&str> {
        self.resolver.host_str()
    }

    pub(super) fn resolver(&self) -> &Url {
        &self.resolver
    }

    /// Open a fresh bidi stream on the cached QUIC connection and exchange the query over it,
    /// reconnecting once if the cached connection has died.
    pub(super) async fn exchange(&self, client: &Client, query: Vec<u8>) -> io::Result<Vec<u8>> {
        log::trace!(
            "DoQ exchange to {}: {}-byte query",
            self.resolver,
            query.len()
        );
        let connection = self.connection(client).await?;
        match connection.open_bidi().await {
            Ok((id, mut stream)) => {
                log::trace!("DoQ query on bidi stream {id} to {}", self.resolver);
                let response = length_prefixed_exchange(&mut stream, &query, true).await;
                log::trace!(
                    "DoQ stream {id} to {} returned {:?}",
                    self.resolver,
                    response.as_ref().map(Vec::len)
                );
                response
            }
            Err(e) => {
                // A dead pooled connection: drop it and re-establish once. A failure to open a
                // stream on a *fresh* connection then propagates.
                log::debug!(
                    "DoQ connection to {} unusable ({e}); reconnecting",
                    self.resolver
                );
                self.invalidate().await;
                let connection = self.connection(client).await?;
                let (id, mut stream) = connection.open_bidi().await?;
                log::trace!(
                    "DoQ query on bidi stream {id} (reconnected) to {}",
                    self.resolver
                );
                length_prefixed_exchange(&mut stream, &query, true).await
            }
        }
    }

    /// The cached QUIC connection to the resolver, establishing one if absent. Holding the lock
    /// across the connect single-flights concurrent queries onto a single connection.
    async fn connection(&self, client: &Client) -> io::Result<QuicConnection> {
        let mut guard = self.connection.lock().await;
        if let Some(connection) = guard.as_ref() {
            return Ok(connection.clone());
        }
        let connection = self.connect(client).await?;
        *guard = Some(connection.clone());
        Ok(connection)
    }

    async fn invalidate(&self) {
        *self.connection.lock().await = None;
    }

    async fn connect(&self, client: &Client) -> io::Result<QuicConnection> {
        let h3 = client.h3().ok_or_else(|| {
            io::Error::new(
                ErrorKind::Unsupported,
                "DoQ requires an HTTP/3-capable client",
            )
        })?;
        let host = self.resolver.host_str().ok_or_else(|| {
            io::Error::new(ErrorKind::InvalidInput, "DoQ resolver URL has no host")
        })?;
        let port = self.resolver.port().unwrap_or(853);

        // The resolver's own host is bootstrapped via the connector's system resolver — it can't be
        // looked up over itself.
        let addr = client
            .connector()
            .resolve(host, port)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| {
                io::Error::new(ErrorKind::NotFound, "no address for DoQ resolver host")
            })?;

        log::debug!("DoQ connecting to {host} at {addr} (alpn doq)");
        let connection = h3
            .connect_with_alpn(host, addr, &[Cow::Borrowed(&b"doq"[..])])
            .await?;
        log::debug!("DoQ connected to {host} at {addr}");
        Ok(connection)
    }
}
