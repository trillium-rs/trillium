use super::ConnInner;
use crate::{
    BufWriter, Buffer, Conn, ConnectionStatus, Error, Headers, HttpContext, KnownHeaderName,
    Method, ProtocolSession, ReceivedBody, Result, Status, TypeSet, Version,
    after_send::AfterSend,
    body::BodyFraming,
    conn::ReceivedBodyState,
    headers::date::current_date_header,
    http_context::ConnectionContext,
    util::{CRLF_CRLF, encoding},
};
use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::{borrow::Cow, io::Write, mem, sync::Arc, time::Instant};

mod request_head;

pub(crate) use request_head::HeadError;

impl<Transport> Conn<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// HTTP/1.x response-header finalization. Parallel to
    /// `finalize_response_headers_h2` and `finalize_response_headers_h3` — keep the
    /// three in sync when changing universal policy (e.g. how Date is set).
    /// Differences are version-intrinsic: chunked Transfer-Encoding and
    /// `Connection: close` are h1-only; the h2/h3 paths strip `H1_ONLY_HEADERS`
    /// and let the framing layer signal end-of-stream.
    pub(super) fn finalize_response_headers_1x(&mut self) {
        if self.inner.status == Some(Status::SwitchingProtocols) {
            return;
        }

        self.inner
            .response_headers
            .try_insert_with(KnownHeaderName::Date, current_date_header);

        if !matches!(
            self.inner.status,
            Some(Status::NotModified | Status::NoContent)
        ) {
            // Upgrade path: don't default to `Content-Length: 0` — body bytes will be
            // written post-handoff. A prelude response body is sent as an open chunked
            // stream (see `Body::keep_open`), so force chunked and drop any Content-Length.
            // With no prelude body, honor an explicit Content-Length; otherwise chunked.
            let has_content_length = if self.inner.upgrade {
                if self
                    .inner
                    .response_body
                    .as_ref()
                    .is_some_and(|b| !b.is_empty())
                {
                    self.inner
                        .response_headers
                        .remove(KnownHeaderName::ContentLength);
                    false
                } else {
                    self.inner
                        .response_headers
                        .has_header(KnownHeaderName::ContentLength)
                }
            } else if let Some(len) = self.body_len() {
                self.inner
                    .response_headers
                    .try_insert(KnownHeaderName::ContentLength, len);
                true
            } else {
                self.inner
                    .response_headers
                    .has_header(KnownHeaderName::ContentLength)
            };

            if self.inner.version == Version::Http1_1 && !has_content_length {
                // Close-delimited framing (RFC 9112 §6.3): `Connection: close` on an
                // unknown-length response opts out of chunked transfer-encoding — the bytes
                // run until the connection closes, carrying neither `Content-Length` nor
                // `Transfer-Encoding`. A prelude body was already normalized to an open
                // chunked stream (see `Body::keep_open`), so an upgrade takes this form
                // only when body-less.
                let prelude_body = self.inner.upgrade
                    && self
                        .inner
                        .response_body
                        .as_ref()
                        .is_some_and(|b| !b.is_empty());
                if !prelude_body && self.response_requests_close() {
                    self.inner
                        .response_headers
                        .remove(KnownHeaderName::TransferEncoding);
                } else {
                    self.inner
                        .response_headers
                        .insert(KnownHeaderName::TransferEncoding, "chunked");
                }
            } else {
                self.inner
                    .response_headers
                    .remove(KnownHeaderName::TransferEncoding);
            }
        }

        if self.inner.context.swansong.state().is_shutting_down() {
            self.inner
                .response_headers
                .insert(KnownHeaderName::Connection, "close");
        }

        // An `Expect: 100-continue` request whose body was never read skips draining before
        // the next request head — draining would block forever against a compliant client
        // that is still waiting for `100 Continue`. Reusing the connection in that state
        // would parse any late-arriving body bytes as the next request head, so close
        // instead: responding with a final status and closing without reading the body is
        // the path RFC 9110 §10.1.1 sanctions for declined expectations.
        if !self.inner.upgrade
            && self.needs_100_continue()
            && self.request_content_length().is_none_or(|len| len > 0)
        {
            self.inner
                .response_headers
                .insert(KnownHeaderName::Connection, "close");
        }
    }

    pub(crate) async fn send(mut self) -> Result<ConnectionStatus<Transport>> {
        let response_buffer_len = self.inner.context.config.response_buffer_len;
        let mut output_buffer = mem::take(&mut self.inner.response_buffer);
        output_buffer.reserve_exact(response_buffer_len);
        self.write_headers(&mut output_buffer)?;

        // Read before the bufwriter borrows `self.transport`, so the body block can consult it.
        let upgrading = self.should_upgrade();

        let max_buf = self.inner.context.config.response_buffer_max_len;
        let mut bufwriter = BufWriter::new_with_buffer(output_buffer, &mut self.transport, max_buf);

        if self.inner.method != Method::Head
            && !matches!(
                self.inner.status,
                Some(Status::NotModified | Status::NoContent)
            )
            && let Some(body) = self.inner.response_body.take()
        {
            // Framing follows the finalized response headers: chunked when
            // `Transfer-Encoding: chunked` is present, raw passthrough (fixed-length or
            // close-delimited) otherwise. An upgrade leaves the chunked stream
            // unterminated for the following upgrade to close.
            let chunked = self
                .inner
                .response_headers
                .has_header(KnownHeaderName::TransferEncoding);

            let framing = if upgrading {
                BodyFraming::Chunked { keep_open: true }
            } else if chunked {
                BodyFraming::Chunked { keep_open: false }
            } else {
                BodyFraming::Raw
            };

            let trailers = body
                .write_into(&mut bufwriter, framing, &self.inner.context.config)
                .await?;

            // The trailer-section and its terminator only exist in chunked framing. When an
            // upgrade follows, the upgrade owns the terminator and the body's trailers (if
            // any) ride onto the `Upgrade`. A close-delimited body has no trailer-section —
            // the connection close is the terminator — so trailers are dropped.
            if !upgrading && chunked {
                // Chunked-trailer-section stitch. `write_into` emitted the last-chunk
                // marker `0\r\n` and stopped there; we own the rest of the framing because
                // trailers are structured `Headers` (not bytes) and the terminating CRLF
                // closes the trailer-section.
                if let Some(trailers) = trailers {
                    log::trace!("sending trailers:\n{trailers}");
                    write_headers_or_trailers(
                        bufwriter.buffer_mut(),
                        &trailers,
                        &self.inner.context,
                    )?;
                    // we don't store the trailers anywhere because the conn is about to be dropped
                }

                write!(bufwriter.buffer_mut(), "\r\n")?;
            }
        }

        bufwriter.flush().await?;

        // A buffer that grew past its configured size goes back to the allocator, so one large
        // response can't pin that memory to an idle keep-alive connection.
        let mut output_buffer = bufwriter.into_buffer();
        if output_buffer.capacity() <= response_buffer_len {
            output_buffer.clear();
            self.inner.response_buffer = output_buffer;
        }

        self.inner.after_send.call(true.into());
        self.finish().await
    }

    pub(super) fn needs_100_continue(&self) -> bool {
        self.inner.request_body_state.is_unread()
            && self.inner.version == Version::Http1_1
            && self
                .inner
                .request_headers
                .eq_ignore_ascii_case(KnownHeaderName::Expect, "100-continue")
    }

    #[allow(clippy::needless_borrow, clippy::needless_borrows_for_generic_args)]
    pub(super) fn build_request_body(&mut self) -> ReceivedBody<'_, Transport> {
        ReceivedBody::new_with_config(
            self.request_content_length(),
            &mut self.inner.buffer,
            &mut self.transport,
            &mut self.inner.request_body_state,
            None,
            encoding(&self.inner.request_headers),
            &self.inner.context.config,
        )
        .with_trailers(&mut self.inner.request_trailers)
        .with_protocol_session(self.inner.protocol_session.clone())
    }

    pub(crate) async fn head(
        transport: &mut Transport,
        buf: &mut Buffer,
        context: &HttpContext,
    ) -> Result<(usize, Instant)> {
        // `total` is the active byte count; `scanned` is how much of it the finder has already
        // covered, so each read only rescans the 3-byte tail overlap plus the new bytes.
        let mut total = 0;
        let mut scanned = 0;
        let mut start_with_read = buf.is_empty();
        let mut instant = None;
        // Leading empty lines discarded before the request-line (RFC 9112 §2.2). Counted against
        // head_max_len so a peer streaming bare CRLFs hits the limit rather than looping forever.
        let mut leading_skipped = 0;
        loop {
            if total + leading_skipped >= context.config.head_max_len {
                return Err(Error::HeadersTooLong);
            }

            // The window never exceeds the remaining allowance, so a reader
            // cannot accumulate more head than head_max_len permits regardless
            // of how finely the peer fragments its writes. Within that ceiling,
            // lending grows geometrically with received bytes instead of
            // front-loading the whole allowance onto every connection.
            // Prefilled bytes are already inside the high-water mark; only
            // freshly read bytes move it.
            let bytes = if start_with_read {
                let remaining = context.config.head_max_len - total - leading_skipped;
                let window = buf.window(
                    remaining.min(
                        context
                            .config
                            .request_buffer_initial_len
                            .max(total.saturating_mul(2))
                            // a zero request_buffer_initial_len must not produce a zero-length
                            // window, which would misread as connection closed
                            .max(1),
                    ),
                );
                let read = if total == 0 {
                    context
                        .swansong
                        .interrupt(transport.read(window))
                        .await
                        .ok_or(Error::Closed)??
                } else {
                    transport.read(window).await?
                };
                buf.advance(read);
                read
            } else {
                start_with_read = true;
                buf.live_len()
            };

            if instant.is_none() {
                instant = Some(Instant::now());
            }

            if bytes == 0 {
                return if total == 0 {
                    Err(Error::Closed)
                } else {
                    Err(Error::InvalidHead)
                };
            }

            total += bytes;

            while buf[..total].starts_with(b"\r\n") {
                buf.ignore_front(2);
                total -= 2;
                scanned = 0;
                leading_skipped += 2;
            }

            let search_start = scanned.max(3) - 3;
            if let Some(index) = CRLF_CRLF.find(&buf[search_start..total]) {
                return Ok((search_start + index + 4, instant.unwrap()));
            }
            scanned = total;
        }
    }

    async fn next(mut self) -> Result<ConnectionStatus<Transport>> {
        // Drain unless the body was already read to its end (draining allocates a buffer even
        // when there is nothing left), or we set up 100-continue and the client never started
        // sending: in that case no body bytes are coming and draining would block.
        if self.inner.request_body_state != ReceivedBodyState::End && !self.needs_100_continue() {
            self.build_request_body().drain().await?;
        }

        let previous_request_headers = self.inner.recycle_h1();

        let head = Self::head(
            &mut self.transport,
            &mut self.inner.buffer,
            &self.inner.context,
        )
        .await;
        let (head_size, start_time) = match head {
            Ok(head) => head,
            Err(Error::Closed) => {
                log::trace!("connection closed by client");
                return Ok(ConnectionStatus::Close);
            }
            Err(e) => return Err(e),
        };
        self.inner.start_time = start_time;

        match self.parse_head(head_size, Some(previous_request_headers)) {
            Ok(conn) => Ok(ConnectionStatus::Conn(conn)),

            Err(HeadError::BadRequest(bad)) => {
                // Box to break the `send -> finish -> next -> send` async-recursion type cycle.
                Box::pin(bad.send()).await?;
                Ok(ConnectionStatus::Close)
            }

            Err(HeadError::Fatal(e)) => Err(e),
        }
    }

    /// True if the outbound headers carry a `Connection: close` token.
    fn response_requests_close(&self) -> bool {
        self.inner
            .response_headers
            .token_iter(KnownHeaderName::Connection)
            .any(|t| t.eq_ignore_ascii_case("close"))
    }

    fn should_close(&self) -> bool {
        // Mirrors the client's `is_keep_alive`.
        let has_token = |headers: &Headers, token: &str| {
            headers
                .token_iter(KnownHeaderName::Connection)
                .any(|t| t.eq_ignore_ascii_case(token))
        };

        if has_token(&self.inner.request_headers, "close")
            || has_token(&self.inner.response_headers, "close")
        {
            true
        } else if has_token(&self.inner.request_headers, "keep-alive")
            && has_token(&self.inner.response_headers, "keep-alive")
        {
            false
        } else {
            self.inner.version == Version::Http1_0
        }
    }

    async fn finish(self) -> Result<ConnectionStatus<Transport>> {
        // Upgrade takes precedence over `Connection: close`: the upgrade owns the rest of the
        // response (its body bytes are written post-handoff), so handing the conn off is the only
        // way to finish sending it. The connection still closes when the upgrade completes.
        if self.should_upgrade() {
            Ok(ConnectionStatus::Upgrade(self.into()))
        } else if self.should_close() {
            Ok(ConnectionStatus::Close)
        } else {
            self.next().await
        }
    }

    fn request_content_length(&self) -> Option<u64> {
        if self
            .inner
            .request_headers
            .has_header(KnownHeaderName::TransferEncoding)
        {
            None
        } else if let Some(content_length) = self.inner.request_headers.content_length() {
            Some(content_length)
        } else if matches!(self.inner.version, Version::Http2 | Version::Http3) {
            // h2 and h3 frame the body via stream-level END_STREAM; there's no equivalent of
            // h1's implicit "no content-length means empty body" default.
            None
        } else {
            Some(0)
        }
    }

    pub(super) fn body_len(&self) -> Option<u64> {
        match self.inner.response_body {
            Some(ref body) => body.len(),
            None => Some(0),
        }
    }

    fn write_headers(&mut self, output_buffer: &mut Vec<u8>) -> Result<()> {
        let status = self.response_status();

        write!(
            output_buffer,
            "{} {} {}\r\n",
            self.inner.version,
            status as u16,
            status.canonical_reason()
        )?;

        self.finalize_headers();

        log::trace!(
            "sending:\n{} {}\n{}",
            self.inner.version,
            status,
            self.inner.response_headers
        );

        write_headers_or_trailers(
            output_buffer,
            &self.inner.response_headers,
            &self.inner.context,
        )?;

        write!(output_buffer, "\r\n")?;

        Ok(())
    }
}

pub(crate) fn write_headers_or_trailers(
    output_buffer: &mut Vec<u8>,
    headers: &Headers,
    context: &HttpContext,
) -> Result<()> {
    let panic_on_invalid = context.config.panic_on_invalid_response_headers;

    for (name, values) in headers {
        if name.is_valid() {
            for value in values {
                if value.is_valid() {
                    write!(output_buffer, "{name}: ")?;
                    output_buffer.extend_from_slice(value.as_ref());
                    write!(output_buffer, "\r\n")?;
                } else if panic_on_invalid {
                    panic!("invalid response header value {value:?} for header {name}");
                } else {
                    log::error!("skipping invalid header value {value:?} for header {name}");
                }
            }
        } else if panic_on_invalid {
            panic!("invalid response header name {name:?}");
        } else {
            log::error!("skipping invalid header with name {name:?}");
        }
    }
    Ok(())
}

impl<T> Conn<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Read and parse the first request head on a connection.
    pub(crate) async fn new_h1(
        context: Arc<HttpContext>,
        mut transport: T,
        initial_bytes: Vec<u8>,
    ) -> Result<Self, HeadError<T>> {
        let mut buffer = Buffer::from(initial_bytes);
        let (head_size, start_time) = Self::head(&mut transport, &mut buffer, &context)
            .await
            .map_err(HeadError::Fatal)?;
        let inner = Box::new(ConnInner::new_h1(Arc::new(context), buffer, start_time));
        Self { inner, transport }.parse_head(head_size, None)
    }
}

impl ConnInner {
    /// A connection's state before its first request head is parsed into it.
    fn new_h1(context: ConnectionContext, buffer: Buffer, start_time: Instant) -> Self {
        // Split each "expected total headers" budget evenly across the known and unknown stores.
        let request_cap = context.config.request_header_initial_capacity;
        let response_cap = context.config.response_header_initial_capacity;
        Self {
            request_headers: Headers::with_capacities(request_cap / 2, request_cap / 2),
            spare_request_headers: None,
            response_headers: Headers::with_capacities(response_cap / 2, response_cap / 2),
            context,
            path: Cow::Borrowed("/"),
            method: Method::Get,
            status: None,
            version: Version::Http1_1,
            state: TypeSet::with_capacity(16),
            response_body: None,
            buffer,
            response_buffer: Vec::new(),
            request_body_state: ReceivedBodyState::End,
            after_send: AfterSend::default(),
            secure: false,
            start_time,
            peer_ip: None,
            authority: None,
            scheme: None,
            protocol_session: ProtocolSession::Http1,
            protocol: None,
            request_trailers: None,
            upgrade: false,
            peer_gone: None,
        }
    }

    /// Ready this connection's state for its next request: per-request fields are reset, while
    /// per-connection state and every reusable allocation carry over. Returns the finished
    /// request's headers, intact, for `parse_head` to salvage values from.
    fn recycle_h1(&mut self) -> Headers {
        // No `..`: a new field must be explicitly carried over or reset.
        let Self {
            context,
            buffer,
            response_buffer: _,
            request_headers,
            spare_request_headers,
            response_headers,
            state,
            path,
            authority,
            scheme,
            // set by `parse_head` from the next request
            method: _,
            version: _,
            request_body_state: _,
            start_time: _,
            status,
            response_body,
            after_send,
            secure,
            peer_ip,
            protocol_session,
            protocol,
            request_trailers,
            upgrade,
            peer_gone,
        } = self;

        // The live region survives (pipelined bytes are the next request); only window space
        // beyond the high-water mark is released, so bytes from an earlier request can never be
        // re-lent while parsing a later one — the cross-principal concern when a proxy coalesces
        // many clients onto one keepalive connection. Capacity is retained; the next lend
        // re-zeroes the span as it grows.
        buffer.truncate();
        state.clear();
        response_headers.clear();
        // The next request sets these too, but they may hold allocations that would otherwise
        // live as long as the connection idles.
        *path = Cow::Borrowed("");
        *authority = None;
        *scheme = None;
        *status = None;
        *response_body = None;
        *after_send = AfterSend::default();
        *secure = false;
        *peer_ip = None;
        *protocol_session = ProtocolSession::Http1;
        *protocol = None;
        *request_trailers = None;
        *upgrade = false;
        *peer_gone = None;

        let parse_target = spare_request_headers.take().unwrap_or_else(|| {
            let cap = context.config.request_header_initial_capacity;
            Headers::with_capacities(cap / 2, cap / 2)
        });
        mem::replace(request_headers, parse_target)
    }
}
