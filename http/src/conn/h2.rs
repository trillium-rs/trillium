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
    /// transport.
    ///
    /// Synchronous — no I/O required at this point because the driver has already decoded the
    /// HEADERS block.
    ///
    /// # Errors
    ///
    /// Returns [`H2ErrorCode::ProtocolError`] for any malformed-request violation: missing
    /// required pseudo-header, empty `:path` outside CONNECT, missing `:authority` for CONNECT,
    /// presence of any HTTP/1-only connection-specific header, `Host` mismatch with
    /// `:authority`, or a `TE` header with a value other than `trailers`.
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
            upgrade: false,
        })
    }

    /// Hand the response off to the [`H2Connection`] driver for framing and transmission.
    ///
    /// On the extended-CONNECT upgrade path — when [`Conn::should_upgrade`] returns true
    /// because the handler has set status 200 on a CONNECT request — this routes through
    /// `submit_upgrade` instead, which signals completion as soon as the HEADERS frame is
    /// on the wire so the upgrade handler can pick up bidirectional I/O.
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

        // Clone rather than take: the Conn is returned with response_headers intact so
        // after_send / drop hooks can observe them.
        let pseudos = PseudoHeaders::default().with_status(self.status.unwrap_or(Status::NotFound));
        let headers = self.response_headers.clone();

        let is_upgrade = self.should_upgrade();
        let result = if is_upgrade {
            log::trace!("h2 stream {stream_id}: send_h2 submitting upgrade");
            h2.submit_upgrade(stream_id, pseudos, headers).await
        } else {
            // HEAD / 304 / 204 responses carry no body. Take unconditionally (so post-send
            // Conn state is consistent) and filter out the body itself for those statuses.
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
    /// surface content-length if known, strip h1-only connection-management headers (forbidden
    /// in h2 per RFC 9113).
    ///
    /// Skips Content-Length insertion on the extended-CONNECT upgrade path: the response is
    /// HEADERS-only on the wire and the stream stays open as a bidirectional byte channel,
    /// so a Content-Length would be both meaningless and misleading.
    ///
    /// Parallel to `finalize_response_headers_1x` (h1) and `finalize_response_headers_h3`
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
