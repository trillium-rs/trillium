use super::Conn;
use crate::h3::H3ClientState;
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

fn h3_to_io(e: H3Error) -> io::Error {
    match e {
        H3Error::Io(io) => io,
        H3Error::Protocol(code) => io::Error::new(ErrorKind::InvalidData, code.to_string()),
        other => io::Error::other(other),
    }
}

impl Conn {
    /// Attempt to execute this request over HTTP/3.
    ///
    /// Returns `Ok(true)` if the request was sent and response headers received via H3.
    /// Returns `Ok(false)` if H3 is unavailable or failed pre-stream, signalling the caller
    /// to fall back to HTTP/1.1. Mid-stream failures are returned as `Err`.
    pub(super) async fn try_exec_h3(&mut self) -> Result<bool> {
        let Some(h3) = self.client.h3().cloned() else {
            return Ok(false);
        };

        let origin = self.url.origin();

        // Check whether we have a usable alt-svc entry for this origin, or whether the caller
        // has hinted that the server supports H3 (skipping the alt-svc dance).
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
        } else {
            return Ok(false);
        };

        // Get an existing pooled connection or establish a new one.
        let entry = match h3
            .get_or_create_quic_conn(&origin, &host, port, self.client.connector(), &self.context)
            .await
        {
            Ok(entry) => entry,
            Err(e) => {
                log::debug!("H3 connect to {host}:{port} failed: {e}, falling back to H1");
                h3.mark_broken(&origin);
                return Ok(false);
            }
        };

        // Extended-CONNECT precondition (RFC 9220 §3): the peer must have advertised
        // SETTINGS_ENABLE_CONNECT_PROTOCOL before we send a `:protocol` HEADERS. Park on the
        // peer's first SETTINGS, then check.
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

        // Open a bidirectional stream for this request/response pair.
        let (stream_id, transport) = match entry.quic_conn.open_bidi().await {
            Ok(t) => t,
            Err(e) => {
                log::debug!("H3 open_bidi failed: {e}, falling back to H1");
                h3.mark_broken(&origin);
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

        // Retain the pool entry on the conn for `into_webtransport`: it needs the dispatcher
        // OnceLock and the QUIC connection to set up the multiplexed session after the 200 OK.
        #[cfg(feature = "webtransport")]
        if self.protocol.as_deref() == Some("webtransport") {
            self.wt_pool_entry = Some(entry.clone());
        }

        // From here on, failures propagate as errors (we've committed to H3).
        self.send_h3_request().await?;
        self.recv_h3_response_headers().await?;

        self.update_alt_svc_from_response(&h3);

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

        // Plain CONNECT omits :scheme and :path (RFC 9114 §4.4); extended CONNECT (RFC 9220)
        // keeps both alongside the :protocol pseudo-header.
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

        if let Some(body) = self.request_body.take() {
            let mut body = body.into_h3();
            bufwriter.copy_from(&mut body, copy_loops_per_yield).await?;
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

        bufwriter.flush().await?;

        // Half-close the write side to signal end of request (RFC 9114 §4.1).
        // For QUIC bidi streams this sends a FIN without closing the read side.
        bufwriter.close().await?;

        Ok(())
    }

    pub(crate) fn finalize_headers_h3(&mut self) -> Result<()> {
        if self.headers_finalized {
            return Ok(());
        }

        // Resolve :authority from explicit Host header (virtual hosting / proxy)
        // or fall back to the URL.
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
            // Plain CONNECT: :scheme and :path are omitted (RFC 9114 §4.4).
            // Extended CONNECT (RFC 9220) keeps both — falls through to the else below.
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

        // Set Content-Length for known-size bodies.
        if let Some(len) = self.body_len()
            && len > 0
        {
            self.request_headers
                .insert(KnownHeaderName::ContentLength, len);
        }

        // Strip connection-specific headers prohibited in HTTP/3 (RFC 9114 §4.2).
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

        // Outer loop: per RFC 9114 §4.1, an HTTP/3 response is zero or more 1xx informational
        // HEADERS frames followed by the final-response HEADERS frame. Per RFC 9110 §15.2 and
        // RFC 8297 §2, headers from interim responses MUST NOT be merged into the final
        // response. We read HEADERS frames in a loop, discarding any whose `:status` is 1xx
        // (except 101 Switching Protocols, which is itself a final response), until we get
        // the final one. Surfacing interim sections to the conn task (for proxy forwarding
        // etc.) is a future enhancement.
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
            break (status, field_section.into_headers().into_owned());
        };

        self.status = status;
        self.response_headers = headers;
        self.response_body_state = ReceivedBodyState::new_h3();

        Ok(())
    }

    /// Scan the response headers for `Alt-Svc` and update the cache accordingly.
    ///
    /// Only called when H3 is configured on the client. Only the first `h3` entry is used;
    /// a value of `clear` removes the cached entry entirely.
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
    buffer.resize(frame_header_len, 0);
    frame.encode(buffer);
    buffer.extend_from_slice(&field_section_buf);

    Ok(())
}
