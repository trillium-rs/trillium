use crate::{
    Buffer, Conn, Headers, KnownHeaderName, Method, Status, TypeSet, Version,
    after_send::AfterSend,
    h2::{H2Connection, H2ErrorCode},
    headers::hpack::{self, FieldSection, PseudoHeaders},
    received_body::ReceivedBodyState,
};
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
    borrow::Cow,
    io,
    sync::Arc,
    time::{Instant, SystemTime},
};

impl<Transport> Conn<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Build a `Conn` from the bits an HTTP/2 stream-open hands the driver: the shared
    /// connection, the stream id, the decoded request field section, and the per-stream
    /// [`H2Transport`][crate::h2::H2Transport].
    ///
    /// Synchronous — no I/O required at this point because the driver has already decoded the
    /// HEADERS block. Returns an [`H2ErrorCode`] for any RFC 9113 §8.1.2 malformed-request
    /// condition; the driver maps these to a stream-level `RST_STREAM` and never spawns a
    /// handler task for the rejected stream.
    ///
    /// # Errors
    ///
    /// Returns [`H2ErrorCode::ProtocolError`] for any §8.1.2 violation: missing required
    /// pseudo-header, empty `:path` outside CONNECT, missing `:authority` for CONNECT, presence
    /// of any HTTP/1-only connection-specific header, `Host` mismatch with `:authority`, or a
    /// `TE` header with a value other than `trailers`.
    pub(crate) fn new_h2(
        h2_connection: Arc<H2Connection>,
        stream_id: u32,
        mut request_headers: FieldSection<'static>,
        transport: Transport,
    ) -> Result<Self, H2ErrorCode> {
        log::trace!("h2 stream {stream_id}: building Conn from\n{request_headers}");
        let pseudo_headers = request_headers.pseudo_headers_mut();

        // §8.1.2.1 response pseudo-header in request: `:status` is the only response-only
        // pseudo and MUST NOT appear in a request.
        if pseudo_headers.status().is_some() {
            return Err(H2ErrorCode::ProtocolError);
        }

        let method = pseudo_headers.take_method();
        let path = pseudo_headers.take_path();
        let authority = pseudo_headers.take_authority();
        let scheme = pseudo_headers.take_scheme();
        let protocol = pseudo_headers.take_protocol();

        let request_headers = request_headers.into_headers().into_owned();

        if let Some(host) = request_headers.get_str(KnownHeaderName::Host)
            && let Some(authority) = &authority
            && host != authority.as_ref()
        {
            return Err(H2ErrorCode::ProtocolError);
        }

        if [
            KnownHeaderName::Connection,
            KnownHeaderName::KeepAlive,
            KnownHeaderName::ProxyConnection,
            KnownHeaderName::TransferEncoding,
            KnownHeaderName::Upgrade,
        ]
        .into_iter()
        .any(|name| request_headers.has_header(name))
        {
            return Err(H2ErrorCode::ProtocolError);
        }

        let method = method.ok_or(H2ErrorCode::ProtocolError)?;

        if method != Method::Connect && scheme.is_none() {
            return Err(H2ErrorCode::ProtocolError);
        }

        let path = match (method, path) {
            (_, Some(path)) if !path.is_empty() => path,
            (Method::Connect, _) => Cow::Borrowed("/"),
            _ => return Err(H2ErrorCode::ProtocolError),
        };

        if method == Method::Connect && authority.is_none() {
            return Err(H2ErrorCode::ProtocolError);
        }

        match request_headers.get_str(KnownHeaderName::Te) {
            None | Some("trailers") => {}
            _ => return Err(H2ErrorCode::ProtocolError),
        }

        let response_headers = h2_connection
            .context()
            .shared_state()
            .get::<Headers>()
            .cloned()
            .unwrap_or_default();

        Ok(Conn {
            context: h2_connection.context(),
            transport,
            request_headers,
            method,
            version: Version::Http2,
            path,
            buffer: Buffer::default(),
            response_headers,
            status: None,
            state: TypeSet::new(),
            response_body: None,
            request_body_state: ReceivedBodyState::new_h2(),
            secure: true,
            after_send: AfterSend::default(),
            start_time: Instant::now(),
            peer_ip: None,
            authority,
            scheme,
            h3_connection: None,
            h3_stream_id: None,
            h2_connection: Some(h2_connection),
            h2_stream_id: Some(stream_id),
            protocol,
            request_trailers: None,
        })
    }

    /// Hand the response off to the [`H2Connection`] driver for framing and transmission.
    ///
    /// Pre-encodes HEADERS into an HPACK byte block on the conn task (the static-or-literal
    /// encoder is stateless), takes the response body off the Conn, and `await`s
    /// [`H2Connection::submit_send`]. The Conn lives across the await — its `Drop` (which
    /// includes things like state-bag teardown and observers) happens *after* the body is
    /// fully on the wire, matching h1/h3's lifecycle.
    ///
    /// On the extended-CONNECT (RFC 8441) upgrade path — i.e.,
    /// [`Conn::should_upgrade`] returns true at this point because the handler has set
    /// status 200 on a CONNECT request — this routes through
    /// [`H2Connection::submit_upgrade`] instead. That signals completion as soon as the
    /// HEADERS frame is on the wire so this function returns and the runtime adapter can
    /// dispatch [`Handler::upgrade`][trillium::Handler::upgrade]; the stream stays open
    /// for the upgrade handler's bidirectional [`H2Transport`][crate::h2::H2Transport]
    /// I/O.
    ///
    /// # Errors
    ///
    /// Returns the [`io::Error`] from the body's `poll_read` or from the underlying transport
    /// if the response failed partway through.
    pub(crate) async fn send_h2(mut self) -> io::Result<Self> {
        self.finalize_response_headers_h2();
        let encoded_headers = encode_headers_h2(
            self.status,
            &self.response_headers,
            self.context.config.response_buffer_len,
        );

        let h2 = self
            .h2_connection
            .clone()
            .ok_or(io::ErrorKind::NotConnected)?;
        let stream_id = self.h2_stream_id.ok_or(io::ErrorKind::NotConnected)?;

        let is_upgrade = self.should_upgrade();
        let result = if is_upgrade {
            log::trace!(
                "h2 stream {stream_id}: send_h2 submitting upgrade ({} header bytes)",
                encoded_headers.len(),
            );
            h2.submit_upgrade(stream_id, encoded_headers).await
        } else {
            // RFC 9110 §3.3.2: HEAD / 304 / 204 responses carry no body. Take unconditionally
            // (so post-send Conn state is consistent) and filter out the body itself for those
            // statuses.
            let allow_body = self.method != Method::Head
                && !matches!(self.status, Some(Status::NotModified | Status::NoContent));
            let body = self.response_body.take().filter(|_| allow_body);
            log::trace!(
                "h2 stream {stream_id}: send_h2 submitting ({} header bytes, body={})",
                encoded_headers.len(),
                body.is_some()
            );
            h2.submit_send(stream_id, encoded_headers, body).await
        };
        log::trace!("h2 stream {stream_id}: send_h2 completed with {result:?}");
        self.after_send.call(result.is_ok().into());
        result.map(|()| self)
    }

    /// Apply h2-flavored finalizations to the response headers: insert a Date header if absent,
    /// surface content-length if known, strip h1-only connection-management headers (which are
    /// forbidden in h2 per RFC 9113 §8.1.2.2).
    ///
    /// Skips Content-Length insertion on the extended-CONNECT upgrade path: the response is
    /// HEADERS-only on the wire and the stream stays open as a bidirectional byte channel,
    /// so a Content-Length would be both meaningless and misleading. RFC 8441 §4 explicitly
    /// notes that the 200 response to an extended CONNECT carries no body in the conventional
    /// sense.
    fn finalize_response_headers_h2(&mut self) {
        self.response_headers
            .try_insert_with(KnownHeaderName::Date, || {
                httpdate::fmt_http_date(SystemTime::now())
            });

        if !self.should_upgrade()
            && !matches!(self.status, Some(Status::NotModified | Status::NoContent))
            && let Some(len) = self.body_len_h2()
        {
            self.response_headers
                .try_insert(KnownHeaderName::ContentLength, len);
        }

        self.response_headers.remove_all([
            KnownHeaderName::Connection,
            KnownHeaderName::TransferEncoding,
            KnownHeaderName::KeepAlive,
            KnownHeaderName::ProxyConnection,
            KnownHeaderName::Upgrade,
        ]);
    }

    fn body_len_h2(&self) -> Option<u64> {
        match self.response_body {
            Some(ref body) => body.len(),
            None => Some(0),
        }
    }
}

/// Encode response headers into an HPACK byte block: `:status` pseudo-header followed by
/// the response Headers map. Static-or-literal — no dynamic-table mutation, safe to do on the
/// conn task without coordination with the driver.
fn encode_headers_h2(
    status: Option<Status>,
    response_headers: &Headers,
    initial_capacity: usize,
) -> Vec<u8> {
    let pseudos = PseudoHeaders::default().with_status(status.unwrap_or(Status::NotFound));
    let field_section = FieldSection::new(pseudos, response_headers);
    let mut buf = Vec::with_capacity(initial_capacity);
    hpack::encode(&field_section, &mut buf);
    buf
}
