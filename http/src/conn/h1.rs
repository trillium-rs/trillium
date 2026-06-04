use crate::{
    BufWriter, Buffer, Conn, ConnectionStatus, Error, Headers, HttpContext, KnownHeaderName,
    Method, ProtocolSession, ReceivedBody, Result, Status, TypeSet, Version, after_send::AfterSend,
    conn::ReceivedBodyState, headers::date::current_date_header,
    util::{encoding, is_tchar},
};
use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use memchr::memmem::Finder;
use std::{borrow::Cow, io::Write, sync::Arc, time::Instant};

/// Outcome of a failed [`Conn::parse_head`].
pub(crate) enum HeadError<Transport> {
    /// A complete but malformed or noncompliant request head. The carried `Conn` owns the
    /// transport and is preset with an error status + `Connection: close`, ready for the caller to
    /// `send()` as the response before closing the connection. Only the `parse` request parser
    /// produces this; the httparse path always closes via [`Fatal`](Self::Fatal).
    #[cfg_attr(not(feature = "parse"), allow(dead_code))]
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
                self.response_headers
                    .insert(KnownHeaderName::TransferEncoding, "chunked");
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
            && let Some(mut body) = self.response_body.take()
        {
            let chunked = body.len().is_none();

            body.ensure_chunked_framing();
            if upgrading {
                // Leave the chunked stream unterminated for the following upgrade to close.
                body.set_keep_open();
            }

            let loops_per_yield = self.context.config.copy_loops_per_yield;

            bufwriter.copy_from(&mut body, loops_per_yield).await?;

            // When an upgrade follows, the upgrade owns the terminator; the body's trailers
            // (if any) ride onto the `Upgrade` and merge with whatever the upgrade handler
            // emits. Skip the terminator stitch here.
            if !upgrading {
                // Chunked-trailer-section stitch. `Body::poll_read` emitted the last-chunk
                // marker `0\r\n` at EOF and stopped there; we own the rest of the framing
                // because trailers are structured `Headers` (not bytes) and the terminating
                // CRLF closes the trailer-section. See `Body::poll_read`'s `len: None`
                // branch for the full rationale.
                if let Some(trailers) = body.trailers() {
                    log::trace!("sending trailers:\n{trailers}");
                    write_headers_or_trailers(bufwriter.buffer_mut(), &trailers, &self.context)?;
                    // we don't store the trailers anywhere because the conn is about to be dropped
                }

                if chunked {
                    write!(bufwriter.buffer_mut(), "\r\n")?;
                }
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
            self.request_content_length().ok().flatten(),
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

    /// Resolve the initial [`ReceivedBodyState`] for the incoming h1 request body from
    /// the parsed headers. h1 requests without explicit framing default to an empty
    /// body — read-to-close on inbound has no sender-side end-of-request signal.
    fn initial_request_body_state(request_headers: &Headers) -> ReceivedBodyState {
        let chunked = request_headers.has_header(KnownHeaderName::TransferEncoding);
        let content_length = if chunked {
            None
        } else {
            request_headers
                .get_str(KnownHeaderName::ContentLength)
                .and_then(|s| s.parse().ok())
                .or(Some(0))
        };
        ReceivedBodyState::new_h1(content_length, chunked)
    }

    fn validate_headers(request_headers: &Headers, version: Version, method: Method) -> Result<()> {
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

        if let Some(content_length) = content_length {
            // RFC 9110 §8.6: exactly one Content-Length whose value is 1*DIGIT. `as_str` is None
            // for multiple values or non-utf8; reject anything that isn't a single strict-digit
            // value rather than coercing it to a 0-length body downstream — that silent coercion
            // is a request-smuggling vector.
            let valid = content_length.as_str().is_some_and(|cl| {
                !cl.is_empty()
                    && cl.bytes().all(|b| b.is_ascii_digit())
                    && cl.parse::<u64>().is_ok()
            });
            if !valid {
                return Err(Error::InvalidHeaderValue(
                    KnownHeaderName::ContentLength.into(),
                ));
            }
        }

        // An HTTP/1.1 request must carry exactly one Host (CONNECT carries its authority in the
        // request target instead), and a present Host must be a bare host[:port] — no userinfo
        // (`@`), path (`/`), comma-list, empty value, or whitespace.
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

    /// Validate the request target. The asterisk-form (`*`) is only valid for `OPTIONS`
    /// (RFC 9112 §3.2.4). When absolute-form supplied an authority, it must agree with the
    /// `Host` header (RFC 9112 §7.2), matching the h2/h3 pseudo-header check.
    #[cfg(feature = "parse")]
    fn validate_request_target(&self) -> Result<()> {
        use super::shared::authority_matches_host;

        // By this point the only well-formed targets are origin-form (path starts with `/`;
        // absolute-form was reconstructed to it in `parse_head`), asterisk-form (`*`, OPTIONS
        // only), and authority-form (CONNECT, whose path was set to `/`). Anything else — a
        // rootless relative target like `foo` — is malformed.
        if &*self.path == "*" {
            if self.method != Method::Options {
                return Err(Error::InvalidHead);
            }
        } else if self.method != Method::Connect && !self.path.starts_with('/') {
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

    #[cfg(not(feature = "parse"))]
    pub(crate) async fn new_internal(
        context: Arc<HttpContext>,
        mut transport: Transport,
        mut buffer: Buffer,
    ) -> Result<Self> {
        use crate::{HeaderName, HeaderValue};
        use httparse::{EMPTY_HEADER, Request};

        let (head_size, start_time) = Self::head(&mut transport, &mut buffer, &context).await?;

        let mut headers = vec![EMPTY_HEADER; context.config.max_headers];
        let mut httparse_req = Request::new(&mut headers);

        let status = httparse_req.parse(&buffer[..]).map_err(|e| match e {
            httparse::Error::HeaderName => Error::InvalidHeaderName,
            httparse::Error::HeaderValue => Error::InvalidHeaderValue("unknown".into()),
            httparse::Error::Status => Error::InvalidStatus,
            httparse::Error::TooManyHeaders => Error::HeadersTooLong,
            httparse::Error::Version => Error::InvalidVersion,
            _ => Error::InvalidHead,
        })?;

        if status.is_partial() {
            return Err(Error::InvalidHead);
        }

        let method = match httparse_req.method {
            Some(method) => match method.parse() {
                Ok(method) => method,
                Err(_) => return Err(Error::UnrecognizedMethod(method.to_string())),
            },
            None => return Err(Error::MissingMethod),
        };

        let version = match httparse_req.version {
            Some(0) => Version::Http1_0,
            Some(1) => Version::Http1_1,
            _ => return Err(Error::InvalidVersion),
        };

        let mut request_headers = Headers::new();
        for header in httparse_req.headers {
            use std::str::FromStr;

            let header_name = HeaderName::from_str(header.name)?;
            let header_value = HeaderValue::from(header.value.to_owned());
            request_headers.append(header_name, header_value);
        }

        Self::validate_headers(&request_headers, version, method)?;

        let mut path = Cow::Owned(
            httparse_req
                .path
                .ok_or(Error::RequestPathMissing)?
                .to_owned(),
        );

        let mut authority = None;

        if method == Method::Connect {
            authority = Some(path);
            path = Cow::Borrowed("/");
        }

        log::trace!("received:\n{method} {path} {version}\n{request_headers}");

        let response_headers = context
            .shared_state()
            .get::<Headers>()
            .cloned()
            .unwrap_or_default();

        buffer.ignore_front(head_size);

        let request_body_state = Self::initial_request_body_state(&request_headers);

        Ok(Self {
            transport,
            request_headers,
            method,
            version,
            path,
            buffer,
            response_headers,
            status: None,
            state: TypeSet::new(),
            response_body: None,
            request_body_state,
            secure: false,
            after_send: AfterSend::default(),
            start_time,
            peer_ip: None,
            context,
            authority,
            scheme: None,
            protocol: None,
            protocol_session: ProtocolSession::Http1,
            request_trailers: None,
            upgrade: false,
        })
    }

    #[cfg(feature = "parse")]
    pub(crate) async fn parse_head(
        context: Arc<HttpContext>,
        mut transport: Transport,
        mut buffer: Buffer,
    ) -> std::result::Result<Self, HeadError<Transport>> {
        let (head_size, start_time) = Self::head(&mut transport, &mut buffer, &context)
            .await
            .map_err(HeadError::Fatal)?;

        let first_line_index = Finder::new(b"\r\n")
            .find(&buffer)
            .ok_or(HeadError::Fatal(Error::InvalidHead))?;

        // `head()` has already required a CRLF-terminated head, so binary garbage (TLS, etc.) never
        // reaches here. A line we still can't tokenize into `method SP target SP version` isn't
        // recognizably a request-line and is closed (Fatal); a tokenizable line always yields a
        // `RequestLine`, with any component-level violation carried in `error` to answer with an
        // error-appropriate status rather than closing.
        let RequestLine {
            method,
            path,
            authority,
            scheme,
            version,
            error: mut first_error,
        } = RequestLine::parse(&buffer[..first_line_index]).map_err(HeadError::Fatal)?;

        let mut request_headers = Headers::new();
        if let Err(e) = request_headers.extend_parse(&buffer[first_line_index + 2..head_size]) {
            first_error.get_or_insert(e);
        }

        let response_headers = context
            .shared_state()
            .get::<Headers>()
            .cloned()
            .unwrap_or_default();

        buffer.ignore_front(head_size);

        let request_body_state = Self::initial_request_body_state(&request_headers);

        let mut conn = Self {
            context,
            transport,
            request_headers,
            method,
            version,
            path,
            buffer,
            response_headers,
            status: None,
            state: TypeSet::new(),
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
            first_error = Self::validate_headers(&conn.request_headers, conn.version, conn.method)
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
            Some(e) => {
                log::debug!("rejecting malformed request: {e}");
                conn.status = Some(status_for_error(&e));
                conn.response_headers
                    .insert(KnownHeaderName::Connection, "close");
                Err(HeadError::BadRequest(Box::new(conn)))
            }
        }
    }

    /// Non-parse wrapper bridging [`new_internal`](Self::new_internal) to the [`HeadError`]
    /// interface. This path doesn't synthesize a `400` response — every error closes the
    /// connection, as it always has.
    #[cfg(not(feature = "parse"))]
    pub(crate) async fn parse_head(
        context: Arc<HttpContext>,
        transport: Transport,
        buffer: Buffer,
    ) -> std::result::Result<Self, HeadError<Transport>> {
        Self::new_internal(context, transport, buffer)
            .await
            .map_err(HeadError::Fatal)
    }

    async fn head(
        transport: &mut Transport,
        buf: &mut Buffer,
        context: &HttpContext,
    ) -> Result<(usize, Instant)> {
        let mut len = 0;
        let mut start_with_read = buf.is_empty();
        let mut instant = None;
        let finder = Finder::new(b"\r\n\r\n");
        loop {
            if len >= context.config.head_max_len {
                return Err(Error::HeadersTooLong);
            }

            let bytes = if start_with_read {
                buf.expand();
                if len == 0 {
                    context
                        .swansong
                        .interrupt(transport.read(buf))
                        .await
                        .ok_or(Error::Closed)??
                } else {
                    transport.read(&mut buf[len..]).await?
                }
            } else {
                start_with_read = true;
                buf.len()
            };

            if instant.is_none() {
                instant = Some(Instant::now());
            }

            let search_start = len.max(3) - 3;
            let search = finder.find(&buf[search_start..]);

            if let Some(index) = search {
                buf.truncate(len + bytes);
                return Ok((search_start + index + 4, instant.unwrap()));
            }

            len += bytes;

            if bytes == 0 {
                return if len == 0 {
                    Err(Error::Closed)
                } else {
                    Err(Error::InvalidHead)
                };
            }
        }
    }

    async fn next(mut self) -> Result<ConnectionStatus<Transport>> {
        // Drain unless we set up 100-continue and the client never started sending: in
        // that case no body bytes are coming and draining would block.
        if !self.needs_100_continue() {
            self.build_request_body().drain().await?;
        }
        match Conn::parse_head(self.context, self.transport, self.buffer).await {
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

    fn should_close(&self) -> bool {
        let has_token = |headers: &Headers, token: &str| {
            headers
                .get_str(KnownHeaderName::Connection)
                .is_some_and(|v| v.split(',').any(|t| t.trim().eq_ignore_ascii_case(token)))
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

    fn request_content_length(&self) -> Result<Option<u64>> {
        if self
            .request_headers
            .has_header(KnownHeaderName::TransferEncoding)
        {
            Ok(None)
        } else if let Some(cl) = self.request_headers.get_str(KnownHeaderName::ContentLength) {
            cl.parse()
                .map(Some)
                .map_err(|_| Error::InvalidHeaderValue(KnownHeaderName::ContentLength.into()))
        } else if matches!(self.version, Version::Http2 | Version::Http3) {
            // h2 and h3 frame the body via stream-level END_STREAM; there's no equivalent of
            // h1's implicit "no content-length means empty body" default.
            Ok(None)
        } else {
            Ok(Some(0))
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
#[cfg(feature = "parse")]
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
#[cfg(feature = "parse")]
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

#[cfg(feature = "parse")]
impl RequestLine {
    /// Parse the request-line bytes (everything before the first CRLF). Returns `Err` only when the
    /// line can't be tokenized into `method SP target SP version` — an unrecognizable line the
    /// caller closes. A tokenizable line always returns `Ok`, with any malformed component
    /// defaulted and recorded in `error`.
    fn parse(first_line: &[u8]) -> Result<Self> {
        let mut spaces = memchr::memchr_iter(b' ', first_line);
        let first_space = spaces.next().ok_or(Error::MissingMethod)?;
        let second_space = spaces.next().ok_or(Error::RequestPathMissing)?;

        let mut error: Option<Error> = None;

        // The method token is case-sensitive. `Method::parse` is lenient (it also
        // backs the h2/h3 `:method` pseudo), so a well-formed but unknown or non-canonically-cased
        // method is unimplemented (→ 501). A method that isn't a valid `token` at all — empty, or
        // carrying a non-tchar octet — is a malformed request-line (→ 400), not an unimplemented
        // method.
        let method_bytes = &first_line[..first_space];
        let method = match Method::parse(method_bytes) {
            Ok(method) if method.as_str().as_bytes() == method_bytes => method,
            _ => {
                error = Some(
                    if !method_bytes.is_empty() && method_bytes.iter().all(|&b| is_tchar(b)) {
                        Error::UnrecognizedMethod(String::from_utf8_lossy(method_bytes).into_owned())
                    } else {
                        Error::InvalidHead
                    },
                );
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
        // A request target is a URI, defined over a limited ASCII set: only printable
        // ASCII (0x21..=0x7e). This rejects controls, space, DEL, and raw non-ASCII (which must be
        // percent-encoded); we don't do full URI-grammar validation beyond that.
        let path = if target.is_empty() || target.iter().any(|&b| !(0x21..=0x7e).contains(&b)) {
            error.get_or_insert(Error::InvalidHead);
            Cow::Borrowed("/")
        } else {
            // every byte is printable ASCII, so this conversion cannot fail
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
                // Absolute-form: split scheme/authority off and reconstruct
                // origin-form so routing sees the same path it would for origin-form, mirroring how
                // h2/h3 carry `:scheme`/`:authority`/`:path`.
                scheme = Some(Cow::Owned(parsed_scheme));
                authority = Some(Cow::Owned(parsed_authority));
                Cow::Owned(parsed_path)
            } else {
                // A non-origin, non-asterisk, non-absolute target — rootless and malformed. Carried
                // as-is and rejected by `validate_request_target`.
                Cow::Owned(target.to_string())
            }
        };

        Ok(Self {
            method,
            path,
            authority,
            scheme,
            version,
            error,
        })
    }
}

/// The response status for a request rejected during head parsing.
#[cfg(feature = "parse")]
fn status_for_error(error: &Error) -> Status {
    match error {
        Error::UnrecognizedMethod(_) => Status::NotImplemented,
        _ => Status::BadRequest,
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
