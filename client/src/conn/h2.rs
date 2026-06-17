use super::Conn;
use crate::pool::{Acquire, PoolEntry, PoolEntryStatus, PrimerGuard};
use std::{
    borrow::Cow,
    fmt::{self, Debug, Formatter},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use trillium_http::{
    Error, KnownHeaderName, Method, ProtocolSession, ReceivedBodyState, Result, Version,
    h2::H2Connection,
    headers::hpack::{FieldSection, PseudoHeaders},
};
use trillium_server_common::{Connector, Transport, url::Origin};

/// Client-side wrapper for a pooled HTTP/2 connection.
///
/// Bundles the shared `Arc<H2Connection>` with the `last_used` instant used for idle-ping
/// decisions.
///
/// Both fields are `Arc`-shared, so clones the pool hands out stay observably equivalent to
/// the original entry (both touch the same `last_used`).
#[derive(Clone)]
pub(crate) struct H2Pooled {
    connection: Arc<H2Connection>,
    last_used: Arc<Mutex<Instant>>,
}

impl Debug for H2Pooled {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("H2Pooled")
            .field("connection", &self.connection)
            .field("last_used", &*self.last_used.lock().unwrap())
            .finish()
    }
}

impl H2Pooled {
    pub(crate) fn new(connection: Arc<H2Connection>) -> Self {
        Self {
            connection,
            last_used: Arc::new(Mutex::new(Instant::now())),
        }
    }

    pub(crate) fn connection(&self) -> &Arc<H2Connection> {
        &self.connection
    }

    fn touch(&self) {
        *self.last_used.lock().unwrap() = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_used.lock().unwrap().elapsed()
    }

    /// How the pool should treat this connection right now: gone ([`Dead`]) once its driver has
    /// stopped, at capacity ([`Busy`]) while it can't open another stream, otherwise reusable
    /// ([`Available`]).
    ///
    /// [`Dead`]: PoolEntryStatus::Dead
    /// [`Busy`]: PoolEntryStatus::Busy
    /// [`Available`]: PoolEntryStatus::Available
    pub(crate) fn classify(&self) -> PoolEntryStatus {
        if !self.connection.swansong().state().is_running() {
            PoolEntryStatus::Dead
        } else if !self.connection.can_open_stream() {
            PoolEntryStatus::Busy
        } else {
            PoolEntryStatus::Available
        }
    }
}

/// Generate an 8-byte opaque payload for an active PING frame. Uses the low 64 bits of
/// system-time nanoseconds since the unix epoch — collisions on a single connection are
/// effectively impossible, and the byte sequence is opaque on the wire.
fn fresh_ping_opaque() -> [u8; 8] {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    nanos.to_be_bytes()
}

impl Conn {
    /// Attempt to execute this request over a pooled HTTP/2 connection.
    ///
    /// Returns `Ok(true)` if a live pooled connection was found and the request was sent on it.
    /// Returns `Ok(false)` if no pooled h2 connection is available, signalling the caller to
    /// fall through to the h1 / fresh-connect path.
    pub(super) async fn try_exec_h2_pooled(&mut self) -> Result<bool> {
        // A prior-knowledge version pin takes precedence: only reuse a pooled h2 connection when
        // h2 is pinned explicitly or no preference was expressed. An h3, h1.1, or h1.0 pin means
        // that protocol.
        if !matches!(self.http_version, None | Some(Version::Http2)) {
            return Ok(false);
        }
        let Some(h2_pool) = self.client.h2_pool() else {
            return Ok(false);
        };
        let origin = self.url.origin();
        let Some(pooled) = h2_pool.peek_candidate_classify(&origin, |p| p.classify()) else {
            return Ok(false);
        };

        if let Some(threshold) = self.client.h2_idle_ping_threshold()
            && pooled.idle_for() > threshold
        {
            let opaque = fresh_ping_opaque();
            let ping = pooled.connection().send_ping(opaque);
            match self
                .client
                .connector()
                .runtime()
                .timeout(self.client.h2_idle_ping_timeout(), ping)
                .await
            {
                Some(Ok(rtt)) => {
                    log::trace!("h2 client liveness ping ack in {rtt:?}");
                }
                other => {
                    log::debug!(
                        "h2 client liveness ping failed ({other:?}); shutting down connection"
                    );
                    pooled.connection().shut_down();
                    return Ok(false);
                }
            }
        }

        pooled.touch();
        self.exec_h2_on_connection(pooled.connection().clone())
            .await?;
        Ok(true)
    }

    /// Open an h2 connection by prior knowledge and execute the request on it.
    ///
    /// Used when the user has set `http_version = Version::Http2`. Over `http://` this is h2c
    /// (cleartext); over `https://` this skips the ALPN-readback dance and starts the h2
    /// driver immediately after the TLS handshake, which is the only way to use h2 with a TLS
    /// connector that doesn't expose `negotiated_alpn` (e.g. native-tls).
    ///
    /// Either way there is no h1 fallback: the preface bytes commit the connection, so a
    /// non-h2-speaking server surfaces as a plain IO error from the h2 driver.
    ///
    /// Concurrent cold-start connects to one origin are coalesced through the pool's in-flight
    /// slot so a burst opens one connection rather than racing N.
    pub(super) async fn exec_h2_prior_knowledge(&mut self) -> Result<()> {
        if let Some(h2_pool) = self.client.h2_pool().cloned() {
            match h2_pool.acquire(self.url.origin(), |p| p.classify()) {
                Acquire::Ready(pooled) => {
                    return self
                        .exec_h2_on_connection(pooled.connection().clone())
                        .await;
                }
                Acquire::Await(cell) => {
                    if let Some(pooled) = cell.wait().await {
                        return self
                            .exec_h2_on_connection(pooled.connection().clone())
                            .await;
                    }
                    // primer produced no connection; connect for ourselves below.
                }
                Acquire::Primer(guard) => {
                    let destination = self.origin_destination().await?;
                    let transport = self.client.connector().connect_to(destination).await?;
                    return self.promote_and_exec_h2(transport, Some(guard)).await;
                }
            }
        }

        let destination = self.origin_destination().await?;
        let transport = self.client.connector().connect_to(destination).await?;
        self.promote_and_exec_h2(transport, None).await
    }

    /// Build an h2 connection over a freshly-connected transport and spawn its driver task. Does
    /// not pool it — recording is the caller's concern (a [`PrimerGuard`] or a direct insert).
    pub(super) fn promote_h2(&self, transport: Box<dyn Transport>) -> Arc<H2Connection> {
        let h2 = H2Connection::new(self.context.clone());
        let initiator = h2.clone().run_client(transport);
        self.client.connector().runtime().spawn(async move {
            if let Err(e) = initiator.await {
                log::debug!("h2 client connection terminated: {e}");
            }
        });
        h2
    }

    /// Promote a freshly-connected transport whose ALPN negotiated `h2` into an h2 connection,
    /// record it, and execute the request on a fresh stream.
    ///
    /// With a `guard`, we are the primer: resolving it pools the connection and hands waiters a
    /// clone. Without one (pooling disabled, or a waiter that fell through), we insert directly
    /// when a pool is configured.
    pub(super) async fn promote_and_exec_h2(
        &mut self,
        transport: Box<dyn Transport>,
        guard: Option<PrimerGuard<Origin, H2Pooled>>,
    ) -> Result<()> {
        let h2 = self.promote_h2(transport);
        let pooled = H2Pooled::new(h2.clone());
        let expiry = self.client.h2_idle_timeout().map(|d| Instant::now() + d);

        match guard {
            Some(guard) => guard.resolve_ready(pooled, expiry),
            None => {
                if let Some(h2_pool) = self.client.h2_pool() {
                    h2_pool.insert(self.url.origin(), PoolEntry::new(pooled, expiry));
                }
            }
        }

        self.exec_h2_on_connection(h2).await
    }

    pub(super) async fn exec_h2_on_connection(&mut self, h2: Arc<H2Connection>) -> Result<()> {
        self.http_version = Some(Version::Http2);
        self.headers_finalized = false;
        self.finalize_headers_h2()?;

        // Headers are cloned (not taken) — `request_headers` is part of the client's public
        // API contract: the caller can re-read sent headers after send for logging, retries,
        // observability. Pseudo-headers are upgraded to 'static via `into_owned`.
        let pseudos = self.build_pseudo_headers()?.into_owned();
        let headers = self.request_headers.clone();
        if log::log_enabled!(log::Level::Trace) {
            let preview = FieldSection::new(pseudos.clone(), &headers);
            log::trace!("sending h2 headers:\n{preview}");
        }

        let (stream_id, transport) = if self.protocol.is_some() {
            // Park on peer SETTINGS before sending `:protocol` — required by RFC 8441 for
            // extended CONNECT. On a pooled connection SETTINGS arrived long ago; on a fresh
            // one we may park briefly. `peer_settings` resolves on either receipt or
            // shutdown, disambiguated via the returned `Option`.
            let Some(settings) = h2.peer_settings().await else {
                return Err(Error::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    "h2 connection closed before peer SETTINGS arrived",
                )));
            };
            if settings.enable_connect_protocol() != Some(true) {
                return Err(Error::ExtendedConnectUnsupported);
            }
            // Extended CONNECT (websocket/webtransport) carries no prelude body. Await the
            // submission so the HEADERS are framed before we read the response, matching the
            // raw-upgrade and h1/h3 sequencing.
            let (stream_id, submit, transport) = h2
                .open_connect_stream(pseudos, headers, None)
                .ok_or_else(|| {
                    Error::Io(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        "h2 connection is shutting down",
                    ))
                })?;
            submit.await?;
            (stream_id, transport)
        } else if self.upgrade {
            // Upgrade path. Same wire shape as extended-CONNECT (HEADERS without END_STREAM,
            // stream stays open), but no `enable_connect_protocol` gating since no
            // `:protocol` is sent. A prelude request body (if any) goes out as DATA before
            // the upgrade transition; `submit.await` returns only once it's on the wire, so
            // the post-`Upgrade` write ordering is well-defined — matching h1/h3.
            let body = self.request_body.take();
            let (stream_id, submit, transport) = h2
                .open_connect_stream(pseudos, headers, body)
                .ok_or_else(|| {
                    Error::Io(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        "h2 connection is shutting down",
                    ))
                })?;
            submit.await?;
            (stream_id, transport)
        } else {
            let body = self.request_body.take();
            let (stream_id, _submit, transport) =
                h2.open_stream(pseudos, headers, body).ok_or_else(|| {
                    Error::Io(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        "h2 connection is shutting down",
                    ))
                })?;
            // Drop `_submit` rather than awaiting it. The driver owns the request body and
            // drives it (DATA + trailers + END_STREAM) independently; the client only needs
            // the response-headers signal to return from `.send()`. If the request fails
            // partway through, the recv path surfaces it as `ConnectionAborted` from
            // `response_headers` — see `H2Connection::open_stream`'s drop-safety note.
            (stream_id, transport)
        };
        log::trace!("h2 client opened stream {stream_id}");

        self.protocol_session = ProtocolSession::Http2 {
            connection: h2.clone(),
            stream_id,
        };
        self.transport = Some(Box::new(transport));

        self.recv_h2_response_headers(&h2, stream_id).await?;

        // Learn any h3 endpoint the response advertises via `Alt-Svc`, matching the h1 path.
        // With h3 configured this makes subsequent requests to this origin prefer h3.
        if let Some(h3) = self.client.h3() {
            self.update_alt_svc_from_response(h3);
        }

        Ok(())
    }

    fn build_pseudo_headers(&self) -> Result<PseudoHeaders<'_>> {
        let mut pseudo = PseudoHeaders::default()
            .with_method(self.method)
            .with_authority(
                self.authority
                    .as_deref()
                    .ok_or(Error::UnexpectedUriFormat)?,
            );

        if self.method != Method::Connect {
            pseudo
                .set_path(Some(
                    self.path.as_deref().ok_or(Error::UnexpectedUriFormat)?,
                ))
                .set_scheme(Some(
                    self.scheme.as_deref().ok_or(Error::UnexpectedUriFormat)?,
                ));
        }

        // set_path/set_scheme above were skipped for CONNECT; layer them on for extended
        // CONNECT.
        if let Some(protocol) = &self.protocol {
            pseudo.set_protocol(Some(protocol.as_ref()));
            if self.method == Method::Connect {
                pseudo
                    .set_path(Some(
                        self.path.as_deref().ok_or(Error::UnexpectedUriFormat)?,
                    ))
                    .set_scheme(Some(
                        self.scheme.as_deref().ok_or(Error::UnexpectedUriFormat)?,
                    ));
            }
        }

        Ok(pseudo)
    }

    pub(super) fn finalize_headers_h2(&mut self) -> Result<()> {
        if self.headers_finalized {
            return Ok(());
        }

        let authority = self
            .request_headers
            .remove(KnownHeaderName::Host)
            .and_then(|h| h.first().map(|v| Cow::Owned(v.to_string())))
            .or_else(|| {
                let host = self.url.host_str()?;
                Some(Cow::Owned(self.url.port().map_or_else(
                    || host.to_string(),
                    |port| format!("{host}:{port}"),
                )))
            })
            .ok_or(Error::UnexpectedUriFormat)?;

        self.authority = Some(authority);

        if let Some(target) = &self.request_target
            && self.method == Method::Options
        {
            self.scheme = Some(Cow::Owned(self.url.scheme().to_string()));
            self.path = Some(target.clone());
        } else if self.method == Method::Connect && self.protocol.is_none() {
            self.scheme = None;
            self.path = None;
        } else {
            self.scheme = Some(Cow::Owned(self.url.scheme().to_string()));
            self.path = Some(Cow::Owned({
                let mut path = self.url.path().to_string();
                if let Some(query) = self.url.query() {
                    path.push('?');
                    path.push_str(query);
                }
                path
            }));
        }

        if self.upgrade || self.protocol.is_some() {
            // Upgrade / extended-CONNECT streams stay open past any prelude body, so a
            // Content-Length would mislead the peer into ending the request body early.
            self.request_headers.remove(KnownHeaderName::ContentLength);
        } else if let Some(len) = self.body_len()
            && len > 0
        {
            self.request_headers
                .insert(KnownHeaderName::ContentLength, len);
        }

        // Any of these may have been added by a prior `finalize_headers_h1` call before we
        // diverted to h2 via ALPN.
        self.request_headers.remove_all([
            KnownHeaderName::Connection,
            KnownHeaderName::TransferEncoding,
            KnownHeaderName::KeepAlive,
            KnownHeaderName::ProxyConnection,
            KnownHeaderName::Upgrade,
            KnownHeaderName::Expect,
        ]);

        self.headers_finalized = true;
        Ok(())
    }

    async fn recv_h2_response_headers(
        &mut self,
        h2: &Arc<H2Connection>,
        stream_id: u32,
    ) -> Result<()> {
        let field_section = h2.response_headers(stream_id).await.map_err(Error::Io)?;
        log::trace!("received h2 response:\n{field_section}");

        self.status = field_section.pseudo_headers().status();
        self.response_headers = field_section.into_headers().into_owned();
        self.response_body_state = ReceivedBodyState::new_h2();

        Ok(())
    }
}
