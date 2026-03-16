use super::Conn;
use crate::h3::H3ClientState;
use futures_lite::AsyncWriteExt;
use std::{borrow::Cow, io};
use trillium_http::{
    Error, KnownHeaderName, Method, ReceivedBodyState, Result, Version,
    h3::{Frame, FrameStream, H3Error},
    headers::qpack::{FieldSection, PseudoHeaders},
};

fn h3_to_io(e: H3Error) -> io::Error {
    match e {
        H3Error::Io(io) => io,
        H3Error::Protocol(code) => io::Error::new(io::ErrorKind::InvalidData, code.to_string()),
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
            .get_or_create_quic_conn(&origin, &host, port, &self.config, &self.server_config)
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

        let mut field_section_buf = Vec::new();
        let field_section = FieldSection::new(pseudo_headers, &self.request_headers);
        log::trace!("sending:\n{field_section}");
        field_section.encode(&mut field_section_buf);

        let headers_frame = Frame::Headers(field_section_buf.len() as u64);
        let mut frame_buf = vec![0u8; headers_frame.encoded_len()];
        headers_frame.encode(&mut frame_buf);

        let transport = self.transport.as_mut().unwrap();
        transport.write_all(&frame_buf).await?;
        transport.write_all(&field_section_buf).await?;

        if let Some(body) = self.request_body.take() {
            match body.len() {
                Some(0) => {}

                Some(len) => {
                    let data_frame = Frame::Data(len);
                    let mut data_buf = vec![0u8; data_frame.encoded_len()];
                    data_frame.encode(&mut data_buf);
                    transport.write_all(&data_buf).await?;
                    futures_lite::io::copy(body, &mut *transport).await?;
                }

                None => {
                    futures_lite::io::copy(body.into_h3(), &mut *transport).await?;
                }
            }
        }

        // Half-close the write side to signal end of request (RFC 9114 §4.1).
        // For QUIC bidi streams this sends a FIN without closing the read side.
        transport.close().await?;

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
