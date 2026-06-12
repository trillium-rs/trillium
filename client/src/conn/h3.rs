use super::Conn;
use crate::h3::{H3ClientState, H3PoolEntry};
use futures_lite::AsyncWriteExt;
use std::{
    borrow::Cow,
    io::{self, ErrorKind},
};
use trillium_http::{
    BufWriter, Error, KnownHeaderName, Method, ProtocolSession, ReceivedBodyState, Result, Status,
    Version,
    h3::{Frame, FrameStream, H3Connection, H3Error},
    headers::qpack::{FieldSection, PseudoHeaders},
};
use trillium_server_common::url::Origin;

fn h3_to_io(e: H3Error) -> io::Error {
    match e {
        H3Error::Io(io) => io,
        H3Error::Protocol(code) => io::Error::new(ErrorKind::InvalidData, code.to_string()),
        other => io::Error::other(other),
    }
}

#[cfg(feature = "hickory")]
impl Conn {
    /// The (host, port) to attempt h3 against when the DoH HTTPS record for this origin advertises
    /// `alpn=h3`, or `None` if it doesn't (or DoH isn't configured).
    ///
    /// Resolving the origin here populates the shared cache, so the subsequent connect resolution
    /// in [`resolve_socket_addrs`](Self::resolve_socket_addrs) is a cache hit in the common case
    /// where the SVCB target is the origin itself.
    async fn svcb_h3_target(&self) -> Result<Option<(String, u16)>> {
        let Some(host) = self.url.host_str() else {
            return Ok(None);
        };
        let Some(origin_port) = self.url.port_or_known_default() else {
            return Ok(None);
        };
        let Some(resolved) = self.resolve(host, origin_port).await? else {
            return Ok(None);
        };
        Ok(resolved
            .services
            .iter()
            .find(|s| s.advertises_h3())
            .map(|binding| {
                let port = binding.port.unwrap_or(origin_port);
                let target = binding.target.clone().unwrap_or_else(|| host.to_string());
                (target, port)
            }))
    }
}

#[cfg(not(feature = "hickory"))]
impl Conn {
    async fn svcb_h3_target(&self) -> Result<Option<(String, u16)>> {
        Ok(None)
    }
}

impl Conn {
    /// Whether h3 may be used for this request: only when h3 is pinned explicitly or no version
    /// preference was expressed (the `Http1_1` auto sentinel). A prior-knowledge h2 or h1.0 pin
    /// takes precedence and forbids h3 even for an origin we know speaks it.
    fn h3_permitted(&self) -> bool {
        self.client.h3().is_some() && matches!(self.http_version, Version::Http3 | Version::Http1_1)
    }

    /// Reuse a live pooled HTTP/3 connection for this origin, if one exists.
    ///
    /// Returns `Ok(true)` if the request went out over a pooled connection. Checked before any
    /// alt-svc/SVCB consideration: holding the connection is itself the decision to use h3, and
    /// alt-svc freshness governs only whether to *establish* a new one. Gating reuse on a
    /// still-usable alt-svc entry would abandon a live connection the moment its (separately
    /// expiring, unrelated) advertisement lapsed.
    pub(super) async fn try_reuse_h3_pool(&mut self) -> Result<bool> {
        if !self.h3_permitted() {
            return Ok(false);
        }
        let h3 = self.client.h3().cloned().unwrap();
        let origin = self.url.origin();
        match h3
            .pool
            .peek_candidate_classify(&origin, |entry| entry.classify())
        {
            Some(entry) => self.exec_h3_on_entry(entry, &h3, &origin).await,
            None => Ok(false),
        }
    }

    /// Establish a new HTTP/3 connection for this request when the origin is known to speak h3
    /// (h3 pinned, a usable Alt-Svc entry, or a DoH HTTPS record advertising `alpn=h3`).
    ///
    /// Returns `Ok(true)` if the request was sent and response headers received over h3. Returns
    /// `Ok(false)` if h3 isn't applicable or the connect failed pre-stream, signalling the caller
    /// to fall back. Mid-stream failures are returned as `Err`.
    pub(super) async fn try_establish_h3(&mut self) -> Result<bool> {
        if !self.h3_permitted() {
            return Ok(false);
        }
        let h3 = self.client.h3().cloned().unwrap();
        let origin = self.url.origin();

        let (host, port) = if self.http_version == Version::Http3 {
            let host = self
                .url
                .host_str()
                .ok_or(Error::UnexpectedUriFormat)?
                .to_string();
            let port = self
                .url
                .port_or_known_default()
                .ok_or(Error::UnexpectedUriFormat)?;
            (host, port)
        } else if let Some(entry) = h3.alt_svc.get(&origin)
            && entry.is_usable()
        {
            (entry.host.clone(), entry.port)
        } else if let Some(target) = self.svcb_h3_target().await? {
            // A DoH HTTPS record advertising `alpn=h3` lets us go straight to h3 on
            // the very first request, without waiting for an Alt-Svc round-trip.
            target
        } else {
            return Ok(false);
        };

        // When DoH is configured, resolve the chosen h3 host through it (fail-closed,
        // cached); otherwise this is empty and the QUIC path resolves via the connector.
        let addrs = self.resolve_socket_addrs(&host, port).await?;

        let entry = match h3
            .get_or_create_quic_conn(
                &origin,
                &host,
                port,
                &addrs,
                self.client.connector(),
                &self.context,
            )
            .await
        {
            Ok(entry) => entry,
            Err(e) => {
                log::debug!("H3 connect to {host}:{port} failed: {e}, falling back to H1");
                h3.mark_broken(&origin);
                return Ok(false);
            }
        };

        self.exec_h3_on_entry(entry, &h3, &origin).await
    }

    /// Execute this request over an established HTTP/3 connection, pooled or freshly connected.
    ///
    /// Returns `Ok(false)` if the stream can't be opened (a dead connection), signalling the caller
    /// to fall back. Once the request has been written we've committed to h3, so later failures
    /// propagate as `Err`.
    async fn exec_h3_on_entry(
        &mut self,
        entry: H3PoolEntry,
        h3: &H3ClientState,
        origin: &Origin,
    ) -> Result<bool> {
        // Park on the peer's first SETTINGS before sending a `:protocol` HEADERS — required
        // by RFC 9220 for extended CONNECT.
        if self.protocol.is_some() {
            let Some(settings) = entry.h3.peer_settings_ready().await else {
                return Err(Error::Closed);
            };
            if !settings.enable_connect_protocol() {
                return Err(Error::ExtendedConnectUnsupported);
            }

            // For webtransport, also verify the WT-specific settings and lazy-init the
            // per-connection dispatcher *before* sending the CONNECT, so any server-initiated
            // WT streams that arrive during the round-trip land in the dispatcher's
            // Buffering state rather than being rejected by the inbound bidi/uni accept task.
            #[cfg(feature = "webtransport")]
            if self.protocol.as_deref() == Some("webtransport") {
                if !settings.enable_webtransport() || !settings.h3_datagram() {
                    return Err(Error::ExtendedConnectUnsupported);
                }
                let _ = entry.dispatcher.get_or_init(
                    trillium_server_common::h3::web_transport::WebTransportDispatcher::new,
                );
            }
        }

        let (stream_id, transport) = match entry.quic_conn.open_bidi().await {
            Ok(t) => t,
            Err(e) => {
                log::debug!("H3 open_bidi failed: {e}, falling back to H1");
                h3.mark_broken(origin);
                return Ok(false);
            }
        };

        self.transport = Some(transport);
        self.http_version = Version::Http3;
        self.finalize_headers_h3()?;
        self.protocol_session = ProtocolSession::Http3 {
            connection: entry.h3.clone(),
            stream_id,
        };

        // Retain the pool entry on the conn — the dispatcher OnceLock and QUIC connection are
        // needed to set up the multiplexed session after the 200 OK.
        #[cfg(feature = "webtransport")]
        if self.protocol.as_deref() == Some("webtransport") {
            self.wt_pool_entry = Some(entry.clone());
        }

        // From here on, failures propagate as errors (we've committed to H3).
        self.send_h3_request().await?;
        self.recv_h3_response_headers().await?;

        self.update_alt_svc_from_response(h3);

        Ok(true)
    }

    async fn send_h3_request(&mut self) -> Result<()> {
        let Some((h3, stream_id)) = self.protocol_session.as_h3_borrowed() else {
            return Err(Error::Closed);
        };
        let mut pseudo_headers = PseudoHeaders::default()
            .with_method(self.method)
            .with_authority(
                self.authority
                    .as_deref()
                    .ok_or(Error::UnexpectedUriFormat)?,
            );

        if self.method != Method::Connect {
            pseudo_headers
                .set_path(Some(
                    self.path.as_deref().ok_or(Error::UnexpectedUriFormat)?,
                ))
                .set_scheme(Some(
                    self.scheme.as_deref().ok_or(Error::UnexpectedUriFormat)?,
                ));
        }

        if let Some(protocol) = &self.protocol {
            pseudo_headers.set_protocol(Some(protocol.as_ref()));
            if self.method == Method::Connect {
                pseudo_headers
                    .set_path(Some(
                        self.path.as_deref().ok_or(Error::UnexpectedUriFormat)?,
                    ))
                    .set_scheme(Some(
                        self.scheme.as_deref().ok_or(Error::UnexpectedUriFormat)?,
                    ));
            }
        }

        let transport = self.transport.as_mut().ok_or(Error::Closed)?;
        let max_buf = self.context.config().response_buffer_max_len();
        let mut bufwriter = BufWriter::new_with_buffer(
            Vec::with_capacity(self.context.config().response_buffer_len()),
            transport,
            max_buf,
        );

        let initial_cap = self.context.config().request_buffer_initial_len();
        let max_peer_field_section_size = None;

        let field_section = FieldSection::new(pseudo_headers, &self.request_headers);
        log::trace!("sending headers:\n{field_section}");

        encode_field_section_h3(
            h3,
            &field_section,
            max_peer_field_section_size,
            initial_cap,
            bufwriter.buffer_mut(),
            stream_id,
        )?;

        let copy_loops_per_yield = self.context.config().copy_loops_per_yield();

        // Upgrade / extended CONNECT (RFC 9220 websocket, webtransport, or a raw CONNECT
        // tunnel via `upgrade`) leaves the write side open: any prelude body streams out as
        // DATA frames, then the caller continues writing DATA, trailing HEADERS, and the FIN
        // via `Upgrade`.
        let is_upgrade = self.upgrade || self.protocol.is_some();

        if let Some(body) = self.request_body.take() {
            let mut body = body.into_h3();
            bufwriter.copy_from(&mut body, copy_loops_per_yield).await?;
            // On an open upgrade stream the body's trailers belong at the eventual close
            // (carried via `Upgrade`), not inline after the prelude.
            if !is_upgrade {
                self.request_trailers = body.trailers();
                if let Some(trailers) = &self.request_trailers {
                    let field_section = FieldSection::new(PseudoHeaders::default(), trailers);
                    log::trace!("sending trailers:\n{field_section}");
                    encode_field_section_h3(
                        h3,
                        &field_section,
                        max_peer_field_section_size,
                        initial_cap,
                        bufwriter.buffer_mut(),
                        stream_id,
                    )?;
                }
            }
        }

        bufwriter.flush().await?;

        // Half-close the write side to signal end of request (QUIC FIN without closing the
        // read side). Skipped for upgrades — the caller sends the FIN via `Upgrade`.
        if !is_upgrade {
            bufwriter.close().await?;
        }

        Ok(())
    }

    pub(crate) fn finalize_headers_h3(&mut self) -> Result<()> {
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
            // OPTIONS * — :path is the explicit target, :scheme is still needed
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

    async fn recv_h3_response_headers(&mut self) -> Result<()> {
        let Some((h3, stream_id)) = self.protocol_session.as_h3_borrowed() else {
            return Err(Error::Closed);
        };

        let transport = self.transport.as_mut().ok_or(Error::Closed)?;
        let mut frame_stream = FrameStream::new(transport, &mut self.buffer);

        // Interim 1xx headers must not be merged into the final response, so discard them.
        // 101 Switching Protocols is itself a final response.
        let (status, headers) = loop {
            let field_section = loop {
                let Some(mut frame) = frame_stream
                    .next()
                    .await
                    .map_err(|e| Error::Io(h3_to_io(e)))?
                else {
                    return Err(Error::Closed);
                };

                // FrameStream auto-skips Unknown frames; anything else here is unexpected
                // but we skip it rather than hard-failing to be tolerant of future frame
                // types.
                if matches!(frame.frame(), Frame::Headers(_)) {
                    let encoded = frame.buffer_payload().await?;

                    break h3
                        .decode_field_section(encoded, stream_id)
                        .await
                        .map_err(|_| Error::InvalidHead)?;
                }
            };
            log::trace!("received:\n{field_section}");

            let status = field_section.pseudo_headers().status();
            if status.is_some_and(|s| s.is_informational() && s != Status::SwitchingProtocols) {
                log::trace!("h3 stream {stream_id}: discarding interim response {status:?}");
                continue;
            }
            // A present Content-Length must be a single run of digits. Reject a malformed
            // framing header rather than coercing it to a body length.
            trillium_http::validate_content_length(
                field_section
                    .headers()
                    .get_values(KnownHeaderName::ContentLength),
            )?;
            break (status, field_section.into_headers().into_owned());
        };

        self.status = status;
        self.response_headers = headers;
        self.response_body_state = ReceivedBodyState::new_h3();

        Ok(())
    }

    /// Scan the response headers for `Alt-Svc` and update the cache accordingly.
    ///
    /// Only the first `h3` entry is used; a value of `clear` removes the cached entry
    /// entirely.
    pub(super) fn update_alt_svc_from_response(&self, h3: &H3ClientState) {
        if let Some(alt_svc) = self.response_headers.get_str(KnownHeaderName::AltSvc) {
            h3.update_alt_svc(alt_svc, &self.url);
        }
    }
}

fn encode_field_section_h3(
    h3: &H3Connection,
    field_section: &FieldSection<'_>,
    max_peer_field_section_size: Option<u64>,
    initial_cap: usize,
    buffer: &mut Vec<u8>,
    stream_id: u64,
) -> io::Result<()> {
    let mut field_section_buf = Vec::with_capacity(initial_cap);
    h3.encode_field_section(field_section, &mut field_section_buf, stream_id)
        .map_err(|error| {
            log::error!("encode error: {error:?}");
            io::Error::other(error)
        })?;

    let size = field_section_buf.len() as u64;
    if let Some(max_size) = max_peer_field_section_size
        && size > max_size
    {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("field section would be longer than peer allows ({size} > {max_size})"),
        ));
    }

    let frame = Frame::Headers(field_section_buf.len() as u64);
    let frame_header_len = frame.encoded_len();
    let start = buffer.len();
    buffer.resize(start + frame_header_len, 0);
    frame.encode(&mut buffer[start..]);
    buffer.extend_from_slice(&field_section_buf);

    Ok(())
}
