use crate::{
    Buffer, Conn, Headers, KnownHeaderName, Method, ProtocolSession, Status, TypeSet, Version,
    after_send::AfterSend,
    h2::{H2Connection, H2ErrorCode},
    headers::hpack::{FieldSection, PseudoHeaders},
    received_body::ReceivedBodyState,
};
use futures_lite::{AsyncRead, AsyncWrite};
use std::{
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
        request_headers: FieldSection<'static>,
        transport: Transport,
    ) -> Result<Self, H2ErrorCode> {
        log::trace!("h2 stream {stream_id}: building Conn from\n{request_headers}");

        let super::ValidatedRequest {
            method,
            path,
            authority,
            scheme,
            protocol,
            request_headers,
        } = super::validate_h2h3_request(request_headers).ok_or(H2ErrorCode::ProtocolError)?;

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
            protocol,
            protocol_session: ProtocolSession::Http2 {
                connection: h2_connection,
                stream_id,
            },
            request_trailers: None,
        })
    }

    /// Hand the response off to the [`H2Connection`] driver for framing and transmission.
    ///
    /// Hands owned (status pseudo, response headers, body) to the driver and `await`s
    /// [`H2Connection::submit_send`]. The driver HPACK-encodes the field section
    /// synchronously at submission pickup; the conn task does no encoding work itself.
    /// The Conn lives across the await — its `Drop` (which
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

        let (h2, stream_id) = self
            .protocol_session
            .as_h2()
            .ok_or(io::ErrorKind::NotConnected)?;

        // Variant A: hand owned (pseudos, headers) to the driver instead of pre-planning.
        // Driver does plan + commit synchronously at submission pickup. Headers are cloned
        // rather than taken — keeps `response_headers()` readable on the returned Conn
        // (mirrors the client-side `request_headers` invariant).
        let pseudos = PseudoHeaders::default().with_status(self.status.unwrap_or(Status::NotFound));
        let headers = self.response_headers.clone();

        let is_upgrade = self.should_upgrade();
        let result = if is_upgrade {
            log::trace!("h2 stream {stream_id}: send_h2 submitting upgrade");
            h2.submit_upgrade(stream_id, pseudos, headers).await
        } else {
            // RFC 9110 §3.3.2: HEAD / 304 / 204 responses carry no body. Take unconditionally
            // (so post-send Conn state is consistent) and filter out the body itself for those
            // statuses.
            let allow_body = self.method != Method::Head
                && !matches!(self.status, Some(Status::NotModified | Status::NoContent));
            let body = self.response_body.take().filter(|_| allow_body);
            log::trace!(
                "h2 stream {stream_id}: send_h2 submitting (body={})",
                body.is_some()
            );
            h2.submit_send(stream_id, pseudos, headers, body).await
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
    ///
    /// Parallel to
    /// [`Conn::finalize_response_headers_1x`][super::Conn::finalize_response_headers_1x]
    /// (h1) and [`Conn::finalize_response_headers_h3`][super::Conn::finalize_response_headers_h3]
    /// (h3); keep the three in sync when changing universal policy.
    fn finalize_response_headers_h2(&mut self) {
        self.response_headers
            .try_insert_with(KnownHeaderName::Date, || {
                httpdate::fmt_http_date(SystemTime::now())
            });

        if !self.should_upgrade()
            && !matches!(self.status, Some(Status::NotModified | Status::NoContent))
            && let Some(len) = self.body_len()
        {
            self.response_headers
                .try_insert(KnownHeaderName::ContentLength, len);
        }

        self.response_headers.remove_all(super::H1_ONLY_HEADERS);
    }
}
