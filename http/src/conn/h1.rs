use crate::{
    BufWriter, Buffer, Conn, ConnectionStatus, Error, Headers, HttpContext, KnownHeaderName,
    Method, ProtocolSession, ReceivedBody, Result, Status, Version,
    after_send::AfterSend,
    body::BodyFraming,
    conn::{ConnParts, ReceivedBodyState, shared::authority_matches_host},
    headers::date::current_date_header,
    util::encoding,
};
use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use memchr::memmem::Finder;
use std::{borrow::Cow, io::Write, time::Instant};

/// Outcome of a failed [`Conn::parse_head`].
pub(crate) enum HeadError<Transport> {
    /// A complete but malformed or noncompliant request head. The carried `Conn` owns the
    /// transport and is preset with an error status + `Connection: close`, ready for the caller to
    /// `send()` as the response before closing the connection.
    BadRequest(Box<Conn<Transport>>),
    /// An unrecoverable error (incomplete head, closed connection, or transport IO). The transport
    /// is gone; the error is propagated to the server layer, which closes the connection.
    Fatal(Error),
}

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
        if self.status == Some(Status::SwitchingProtocols) {
            return;
        }

        self.response_headers
            .try_insert_with(KnownHeaderName::Date, current_date_header);

        if !matches!(self.status, Some(Status::NotModified | Status::NoContent)) {
            // Upgrade path: don't default to `Content-Length: 0` — body bytes will be
            // written post-handoff. A prelude response body is sent as an open chunked
            // stream (see `Body::keep_open`), so force chunked and drop any Content-Length.
            // With no prelude body, honor an explicit Content-Length; otherwise chunked.
            let has_content_length = if self.upgrade {
                if self.response_body.as_ref().is_some_and(|b| !b.is_empty()) {
                    self.response_headers.remove(KnownHeaderName::ContentLength);
                    false
                } else {
                    self.response_headers
                        .has_header(KnownHeaderName::ContentLength)
                }
            } else if let Some(len) = self.body_len() {
                self.response_headers
                    .try_insert(KnownHeaderName::ContentLength, len);
                true
            } else {
                self.response_headers
                    .has_header(KnownHeaderName::ContentLength)
            };

            if self.version == Version::Http1_1 && !has_content_length {
                // Close-delimited framing (RFC 9112 §6.3): `Connection: close` on an
                // unknown-length response opts out of chunked transfer-encoding — the body
                // runs until the connection closes, carrying neither `Content-Length` nor
                // `Transfer-Encoding`. Upgrades own their framing separately (chunked
                // keep-open prelude).
                if !self.upgrade && self.response_requests_close() {
                    self.response_headers
                        .remove(KnownHeaderName::TransferEncoding);
                } else {
                    self.response_headers
                        .insert(KnownHeaderName::TransferEncoding, "chunked");
                }
            } else {
                self.response_headers
                    .remove(KnownHeaderName::TransferEncoding);
            }
        }

        if self.context.swansong.state().is_shutting_down() {
            self.response_headers
                .insert(KnownHeaderName::Connection, "close");
        }
    }

    pub(crate) async fn send(mut self) -> Result<ConnectionStatus<Transport>> {
        let mut output_buffer = Vec::with_capacity(self.context.config.response_buffer_len);
        self.write_headers(&mut output_buffer)?;

        // Read before the bufwriter borrows `self.transport`, so the body block can consult it.
        let upgrading = self.should_upgrade();

        let max_buf = self.context.config.response_buffer_max_len;
        let mut bufwriter = BufWriter::new_with_buffer(output_buffer, &mut self.transport, max_buf);

        if self.method != Method::Head
            && !matches!(self.status, Some(Status::NotModified | Status::NoContent))
            && let Some(body) = self.response_body.take()
        {
            // Framing follows the finalized response headers: chunked when
            // `Transfer-Encoding: chunked` is present, raw passthrough (fixed-length or
            // close-delimited) otherwise. An upgrade leaves the chunked stream
            // unterminated for the following upgrade to close.
            let chunked = self
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
                .write_into(&mut bufwriter, framing, &self.context.config)
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
                    write_headers_or_trailers(bufwriter.buffer_mut(), &trailers, &self.context)?;
                    // we don't store the trailers anywhere because the conn is about to be dropped
                }

                write!(bufwriter.buffer_mut(), "\r\n")?;
            }
        }

        bufwriter.flush().await?;

        self.after_send.call(true.into());
        self.finish().await
    }

    pub(super) fn needs_100_continue(&self) -> bool {
        self.request_body_state.is_unread()
            && self.version == Version::Http1_1
            && self
                .request_headers
                .eq_ignore_ascii_case(KnownHeaderName::Expect, "100-continue")
    }

    #[allow(clippy::needless_borrow, clippy::needless_borrows_for_generic_args)]
    pub(super) fn build_request_body(&mut self) -> ReceivedBody<'_, Transport> {
        ReceivedBody::new_with_config(
            self.request_content_length(),
            &mut self.buffer,
            &mut self.transport,
            &mut self.request_body_state,
            None,
            encoding(&self.request_headers),
            &self.context.config,
        )
        .with_trailers(&mut self.request_trailers)
        .with_protocol_session(self.protocol_session.clone())
    }

    fn validate_headers_h1(&self) -> Result<()> {
        let Self {
            ref request_headers,
            version,
            method,
            ..
        } = *self;
        let content_length = request_headers.get_values(KnownHeaderName::ContentLength);
        let transfer_encoding = request_headers.get_values(KnownHeaderName::TransferEncoding);

        if let Some(te) = transfer_encoding
            && te
                .as_str()
                .is_none_or(|te_str| !te_str.eq_ignore_ascii_case("chunked"))
        {
            return Err(Error::UnexpectedHeader(
                KnownHeaderName::TransferEncoding.into(),
            ));
        }

        if content_length.is_some() && transfer_encoding.is_some() {
            return Err(Error::UnexpectedHeader(
                KnownHeaderName::ContentLength.into(),
            ));
        }

        crate::util::validate_content_length(content_length)?;

        if let Some(expect) = request_headers.get_values(KnownHeaderName::Expect) {
            let all_continue = expect.iter().all(|value| {
                value.as_str().is_some_and(|value| {
                    value
                        .split(',')
                        .all(|token| token.trim().eq_ignore_ascii_case("100-continue"))
                })
            });
            if !all_continue {
                return Err(Error::ExpectationFailed);
            }
        }

        match request_headers.get_values(KnownHeaderName::Host) {
            None => {
                if version == Version::Http1_1 && method != Method::Connect {
                    return Err(Error::HeaderMissing(KnownHeaderName::Host.into()));
                }
            }
            Some(host) => {
                let valid = host.as_str().is_some_and(|host| {
                    !host.is_empty()
                        && !host
                            .bytes()
                            .any(|b| matches!(b, b'@' | b'/' | b',') || b <= b' ')
                });
                if !valid {
                    return Err(Error::InvalidHeaderValue(KnownHeaderName::Host.into()));
                }
            }
        }

        Ok(())
    }

    /// Validate the request target. The asterisk-form (`*`) is only valid for `OPTIONS`. When
    /// absolute-form supplied an authority, it must agree with the `Host` header.
    fn validate_request_target(&self) -> Result<()> {
        if &*self.path == "*" {
            if self.method != Method::Options {
                return Err(Error::InvalidHead);
            }
        } else if self.method == Method::Connect {
            // authority-form; path was normalized to `/` in parse_head — nothing to validate here.
        } else if self.path.starts_with('/') {
            if self.path.contains(['#', '\\']) {
                return Err(Error::InvalidHead);
            }
        } else {
            return Err(Error::InvalidHead);
        }

        if self.method != Method::Connect
            && let Some(authority) = &self.authority
            && let Some(host) = self.request_headers.get_str(KnownHeaderName::Host)
            && !authority_matches_host(authority, host, self.scheme.as_deref())
        {
            return Err(Error::InvalidHeaderValue(KnownHeaderName::Host.into()));
        }

        Ok(())
    }

    async fn head(
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
        let finder = Finder::new(b"\r\n\r\n");
        loop {
            if total + leading_skipped >= context.config.head_max_len {
                return Err(Error::HeadersTooLong);
            }

            let bytes = if start_with_read {
                buf.expand();
                if total == 0 {
                    context
                        .swansong
                        .interrupt(transport.read(buf))
                        .await
                        .ok_or(Error::Closed)??
                } else {
                    transport.read(&mut buf[total..]).await?
                }
            } else {
                start_with_read = true;
                buf.len()
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
            if let Some(index) = finder.find(&buf[search_start..total]) {
                buf.truncate(total);
                return Ok((search_start + index + 4, instant.unwrap()));
            }
            scanned = total;
        }
    }

    async fn next(mut self) -> Result<ConnectionStatus<Transport>> {
        // Drain unless we set up 100-continue and the client never started sending: in
        // that case no body bytes are coming and draining would block.
        if !self.needs_100_continue() {
            self.build_request_body().drain().await?;
        }

        match ConnParts::from(self).parse_head().await {
            Ok(conn) => Ok(ConnectionStatus::Conn(conn)),

            Err(HeadError::BadRequest(bad)) => {
                // Box to break the `send -> finish -> next -> send` async-recursion type cycle.
                Box::pin(bad.send()).await?;
                Ok(ConnectionStatus::Close)
            }

            Err(HeadError::Fatal(Error::Closed)) => {
                log::trace!("connection closed by client");
                Ok(ConnectionStatus::Close)
            }

            Err(HeadError::Fatal(e)) => Err(e),
        }
    }

    /// True if the outbound headers carry a `Connection: close` token.
    fn response_requests_close(&self) -> bool {
        self.response_headers
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

        if has_token(&self.request_headers, "close") || has_token(&self.response_headers, "close") {
            true
        } else if has_token(&self.request_headers, "keep-alive")
            && has_token(&self.response_headers, "keep-alive")
        {
            false
        } else {
            self.version == Version::Http1_0
        }
    }

    async fn finish(self) -> Result<ConnectionStatus<Transport>> {
        if self.should_close() {
            Ok(ConnectionStatus::Close)
        } else if self.should_upgrade() {
            Ok(ConnectionStatus::Upgrade(self.into()))
        } else {
            self.next().await
        }
    }

    fn request_content_length(&self) -> Option<u64> {
        if self
            .request_headers
            .has_header(KnownHeaderName::TransferEncoding)
        {
            None
        } else if let Some(content_length) = self.request_headers.content_length() {
            Some(content_length)
        } else if matches!(self.version, Version::Http2 | Version::Http3) {
            // h2 and h3 frame the body via stream-level END_STREAM; there's no equivalent of
            // h1's implicit "no content-length means empty body" default.
            None
        } else {
            Some(0)
        }
    }

    pub(super) fn body_len(&self) -> Option<u64> {
        match self.response_body {
            Some(ref body) => body.len(),
            None => Some(0),
        }
    }

    fn write_headers(&mut self, output_buffer: &mut Vec<u8>) -> Result<()> {
        let status = self.response_status();

        write!(
            output_buffer,
            "{} {} {}\r\n",
            self.version,
            status as u16,
            status.canonical_reason()
        )?;

        self.finalize_headers();

        log::trace!(
            "sending:\n{} {}\n{}",
            self.version,
            status,
            self.response_headers
        );

        write_headers_or_trailers(output_buffer, &self.response_headers, &self.context)?;

        write!(output_buffer, "\r\n")?;

        Ok(())
    }
}

/// Split an absolute-form request target (`scheme "://" authority
/// path-abempty [ "?" query ]`) into `(scheme, authority, origin-form-path)`. Returns
/// `None` when `target` isn't absolute-form (no `://`, an invalid scheme, or an empty
/// authority), leaving the caller to treat it as malformed.
fn split_absolute_form(target: &str) -> Option<(String, String, String)> {
    let (scheme, rest) = target.split_once("://")?;
    let mut scheme_bytes = scheme.bytes();
    let valid_scheme = scheme_bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
        && scheme_bytes.all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
    if !valid_scheme {
        return None;
    }

    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return None;
    }

    let path = &rest[authority_end..];
    // An empty path-abempty (or one that begins with the query/fragment) reconstructs to "/".
    let path = if path.is_empty() || path.starts_with(['?', '#']) {
        format!("/{path}")
    } else {
        path.to_string()
    };

    Some((scheme.to_string(), authority.to_string(), path))
}

/// The parsed components of an HTTP/1.x request-line (`method SP request-target SP HTTP-version`).
struct RequestLine {
    method: Method,
    path: Cow<'static, str>,
    authority: Option<Cow<'static, str>>,
    scheme: Option<Cow<'static, str>>,
    version: Version,
    /// The first component-level violation, if any. `None` is a clean request-line; a `Some` is
    /// carried up so the caller answers with a status derived from the error rather than closing.
    error: Option<Error>,
}

impl RequestLine {
    fn parse(first_line: &[u8]) -> Self {
        let mut spaces = memchr::memchr_iter(b' ', first_line);
        let Some(first_space) = spaces.next() else {
            return Self::malformed(Error::MissingMethod);
        };
        let Some(second_space) = spaces.next() else {
            return Self::malformed(Error::RequestPathMissing);
        };

        let mut error: Option<Error> = None;

        let method_bytes = &first_line[..first_space];
        let method = match Method::parse(method_bytes) {
            Ok(method) => method,
            Err(e) => {
                error = Some(e);
                Method::Get
            }
        };

        let version = match Version::parse(&first_line[second_space + 1..]) {
            Ok(version) => version,
            Err(e) => {
                error.get_or_insert(e);
                Version::Http1_1
            }
        };

        let target = &first_line[first_space + 1..second_space];
        let mut authority = None;
        let mut scheme = None;

        let path = if target.is_empty() || target.iter().any(|&b| !(0x21..=0x7e).contains(&b)) {
            error.get_or_insert(Error::InvalidHead);
            Cow::Borrowed("/")
        } else {
            let target = std::str::from_utf8(target).unwrap_or("/");
            if method == Method::Connect {
                authority = Some(Cow::Owned(target.to_string()));
                Cow::Borrowed("/")
            } else if target == "*" {
                Cow::Borrowed("*")
            } else if target.starts_with('/') {
                Cow::Owned(target.to_string())
            } else if let Some((parsed_scheme, parsed_authority, parsed_path)) =
                split_absolute_form(target)
            {
                scheme = Some(Cow::Owned(parsed_scheme));
                authority = Some(Cow::Owned(parsed_authority));
                Cow::Owned(parsed_path)
            } else {
                Cow::Owned(target.to_string())
            }
        };

        Self {
            method,
            path,
            authority,
            scheme,
            version,
            error,
        }
    }

    fn malformed(error: Error) -> Self {
        Self {
            method: Method::Get,
            path: Cow::Borrowed("/"),
            authority: None,
            scheme: None,
            version: Version::Http1_1,
            error: Some(error),
        }
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

impl<T> ConnParts<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    pub(crate) async fn parse_head(self) -> Result<Conn<T>, HeadError<T>> {
        let Self {
            mut buffer,
            state,
            mut request_headers,
            mut response_headers,
            context,
            mut transport,
        } = self;

        let (head_size, start_time) = Conn::head(&mut transport, &mut buffer, &context)
            .await
            .map_err(HeadError::Fatal)?;

        let first_line_index = Finder::new(b"\r\n")
            .find(&buffer)
            .ok_or(HeadError::Fatal(Error::InvalidHead))?;

        let RequestLine {
            method,
            path,
            authority,
            scheme,
            version,
            error: mut first_error,
        } = RequestLine::parse(&buffer[..first_line_index]);

        if let Err(e) = request_headers.extend_parse(&buffer[first_line_index + 2..head_size]) {
            first_error.get_or_insert(e);
        }

        if let Some(default_headers) = context.shared_state().get().cloned() {
            response_headers.insert_all(default_headers);
        }

        buffer.ignore_front(head_size);

        let request_body_state = Self::initial_request_body_state(&request_headers);

        let mut conn = Conn {
            context,
            transport,
            request_headers,
            method,
            version,
            path,
            buffer,
            response_headers,
            status: None,
            state,
            response_body: None,
            request_body_state,
            secure: false,
            after_send: AfterSend::default(),
            start_time,
            peer_ip: None,
            authority,
            scheme,
            protocol: None,
            protocol_session: ProtocolSession::Http1,
            request_trailers: None,
            upgrade: false,
        };

        // Cross-header and request-target rules only apply to an otherwise-clean parse; once we
        // already have a violation we're synthesizing a response regardless.
        if first_error.is_none() {
            first_error = conn
                .validate_headers_h1()
                .and_then(|()| conn.validate_request_target())
                .err();
        }

        match first_error {
            None => {
                log::trace!(
                    "received:\n{} {} {}\n{}",
                    conn.method,
                    conn.path,
                    conn.version,
                    conn.request_headers
                );
                Ok(conn)
            }
            Some(ref e) => {
                log::debug!("rejecting malformed request: {e}");
                conn.status = Some(e.into());
                conn.response_headers
                    .insert(KnownHeaderName::Connection, "close");
                Err(HeadError::BadRequest(Box::new(conn)))
            }
        }
    }

    /// Resolve the initial [`ReceivedBodyState`] for the incoming h1 request body from
    /// the parsed headers. h1 requests without explicit framing default to an empty
    /// body — read-to-close on inbound has no sender-side end-of-request signal.
    fn initial_request_body_state(request_headers: &Headers) -> ReceivedBodyState {
        let chunked = request_headers.has_header(KnownHeaderName::TransferEncoding);
        let content_length = if chunked {
            None
        } else {
            request_headers.content_length().or(Some(0))
        };
        ReceivedBodyState::new_h1(content_length, chunked)
    }
}
