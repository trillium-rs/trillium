use super::Conn;
use crate::h3::H3ClientState;
use futures_lite::AsyncWriteExt;
use std::{
    borrow::Cow,
    io::{self, ErrorKind},
};
use trillium_http::{
    BufWriter, Error, KnownHeaderName, Method, ReceivedBodyState, Result, Version,
    h3::{Frame, FrameStream, H3Error},
    headers::qpack::{FieldSection, PseudoHeaders},
};

fn h3_to_io(e: H3Error) -> io::Error {
    match e {
        H3Error::Io(io) => io,
        H3Error::Protocol(code) => io::Error::new(ErrorKind::InvalidData, code.to_string()),
    }
}

impl Conn {
    /// Attempt to execute this request over HTTP/3.
    ///
    /// Returns `Ok(true)` if the request was sent and response headers received via H3.
    /// Returns `Ok(false)` if H3 is unavailable or failed pre-stream, signalling the caller
    /// to fall back to HTTP/1.1. Mid-stream failures are returned as `Err`.
    pub(super) async fn try_exec_h3(&mut self, h3: &H3ClientState) -> Result<bool> {
        let origin = self.url.origin();

        // Check whether we have a usable alt-svc entry for this origin.
        let (host, port) = if let Some(entry) = h3.alt_svc.get(&origin)
            && entry.is_usable()
        {
            (entry.host.clone(), entry.port)
        } else {
            return Ok(false);
        };

        // Get an existing pooled connection or establish a new one.
        let quic_conn = match h3
            .get_or_create_quic_conn(&origin, &host, port, &self.config, &self.context)
            .await
        {
            Ok(conn) => conn,
            Err(e) => {
                log::debug!("H3 connect to {host}:{port} failed: {e}, falling back to H1");
                h3.mark_broken(&origin);
                return Ok(false);
            }
        };

        // Open a bidirectional stream for this request/response pair.
        let (_, transport) = match quic_conn.open_bidi().await {
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

        // From here on, failures propagate as errors (we've committed to H3).
        self.send_h3_request().await?;
        self.recv_h3_response_headers().await?;

        Ok(true)
    }

    async fn send_h3_request(&mut self) -> Result<()> {
        let mut pseudo_headers = PseudoHeaders::default()
            .with_method(self.method)
            .with_authority(
                self.authority
                    .as_deref()
                    .ok_or(Error::UnexpectedUriFormat)?,
            );

        // CONNECT omits :scheme and :path (RFC 9114 §4.4)
        if self.method != Method::Connect {
            pseudo_headers
                .set_path(Some(
                    self.path.as_deref().ok_or(Error::UnexpectedUriFormat)?,
                ))
                .set_scheme(Some(
                    self.scheme.as_deref().ok_or(Error::UnexpectedUriFormat)?,
                ));
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
            &field_section,
            max_peer_field_section_size,
            initial_cap,
            bufwriter.buffer_mut(),
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
                    &field_section,
                    max_peer_field_section_size,
                    initial_cap,
                    bufwriter.buffer_mut(),
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
        } else if self.method == Method::Connect {
            // CONNECT omits :scheme and :path in H3 (RFC 9114 §4.4)
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
        let transport = self.transport.as_mut().ok_or(Error::Closed)?;
        let mut frame_stream = FrameStream::new(transport, &mut self.buffer);
        let field_section = loop {
            let Some(mut frame) = frame_stream
                .next()
                .await
                .map_err(|e| Error::Io(h3_to_io(e)))?
            else {
                return Err(Error::Closed);
            };

            // Per RFC 9114 §4.1, the first frame on a response stream MUST be HEADERS.
            // FrameStream auto-skips Unknown frames; anything else here is unexpected but
            // we skip it rather than hard-failing to be tolerant of future frame types.
            if matches!(frame.frame(), Frame::Headers(_)) {
                let payload = frame.buffer_payload().await?;
                break FieldSection::decode(payload).map_err(|_| Error::InvalidHead)?;
            }
        };
        log::trace!("received:\n{field_section}");

        self.status = field_section.pseudo_headers().status();
        self.response_headers = field_section.into_headers().into_owned();
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
    field_section: &FieldSection<'_>,
    max_peer_field_section_size: Option<u64>,
    initial_cap: usize,
    buffer: &mut Vec<u8>,
) -> io::Result<()> {
    let mut field_section_buf = Vec::with_capacity(initial_cap);
    field_section.encode(&mut field_section_buf);

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
