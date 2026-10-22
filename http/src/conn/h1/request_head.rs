use crate::{
    Conn, Error, Headers, KnownHeaderName, Method, Result, Status, Version,
    conn::{ConnInner, ReceivedBodyState, shared::authority_matches_host},
    util::CRLF,
};
use futures_lite::{AsyncRead, AsyncWrite};
use std::borrow::Cow;

/// Outcome of a failed [`Conn::parse_head`].
pub(crate) enum HeadError<Transport> {
    /// A complete but malformed or noncompliant request head. The carried `Conn` owns the
    /// transport and is preset with an error status + `Connection: close`, ready for the caller to
    /// `send()` as the response before closing the connection.
    BadRequest(Conn<Transport>),
    /// An unrecoverable error (incomplete head, closed connection, or transport IO). The transport
    /// is gone; the error is propagated to the server layer, which closes the connection.
    Fatal(Error),
}

impl<T> Conn<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Parse the request head occupying the first `head_size` bytes of the buffer into this
    /// conn. `previous_request_headers` holds the prior request's headers on this connection,
    /// whose values the parse may reuse.
    pub(super) fn parse_head(
        mut self,
        head_size: usize,
        mut previous_request_headers: Option<Headers>,
    ) -> Result<Self, HeadError<T>> {
        let inner = &mut *self.inner;
        let first_line_index = CRLF
            .find(&inner.buffer)
            .ok_or(HeadError::Fatal(Error::InvalidHead))?;

        let RequestLine {
            method,
            path,
            authority,
            scheme,
            version,
            error: mut first_error,
            unrecognized_method,
        } = RequestLine::parse(&inner.buffer[..first_line_index]);
        inner.method = method;
        inner.path = path;
        inner.authority = authority;
        inner.scheme = scheme;
        inner.version = version;

        let header_bytes = &inner.buffer[first_line_index + 2..head_size];
        let parsed = match &mut previous_request_headers {
            Some(previous) => {
                let parsed = inner
                    .request_headers
                    .extend_parse_reusing(header_bytes, previous);
                previous.clear();
                parsed
            }
            None => inner.request_headers.extend_parse(header_bytes),
        };
        inner.spare_request_headers = previous_request_headers;
        if let Err(e) = parsed {
            first_error.get_or_insert(e);
        }

        if let Some(default_headers) = inner.context.shared_state().get().cloned() {
            inner.response_headers.insert_all(default_headers);
        }

        inner.buffer.ignore_front(head_size);
        inner.request_body_state = initial_request_body_state(&inner.request_headers);

        // Cross-header and request-target rules only apply to an otherwise-clean parse; once we
        // already have a violation we're synthesizing a response regardless.
        if first_error.is_none() {
            first_error = self
                .validate_headers_h1()
                .and_then(|()| self.validate_request_target())
                .err();
        }

        match first_error.or_else(|| unrecognized_method.map(Error::UnrecognizedMethod)) {
            None => {
                log::trace!(
                    "received:\n{} {} {}\n{}",
                    self.inner.method,
                    self.inner.path,
                    self.inner.version,
                    self.inner.request_headers
                );
                Ok(self)
            }
            Some(ref e) => Err(HeadError::BadRequest(self.reject(e))),
        }
    }

    /// Ready this conn to answer a request head that won't be handled, with the status for
    /// `error`.
    pub(super) fn reject(self, error: &Error) -> Self {
        log::debug!("rejecting request: {error}");
        self.close_with(error.into())
    }

    /// Ready this conn to answer a request head that outgrew `head_max_len`, with the status for
    /// whichever part of the head was still incomplete when it did.
    pub(super) fn reject_oversized_head(self) -> Self {
        let status = oversized_head_status(&self.inner.buffer);
        log::debug!("rejecting request: head longer than allowed ({status})");
        self.close_with(status)
    }

    /// Respond with `status` and `Connection: close`, since the bytes after a rejected head can't
    /// be trusted to frame another request.
    fn close_with(mut self, status: Status) -> Self {
        self.inner.status = Some(status);
        self.inner
            .response_headers
            .insert(KnownHeaderName::Connection, "close");
        self
    }

    fn validate_headers_h1(&self) -> Result<()> {
        let ConnInner {
            ref request_headers,
            version,
            method,
            ..
        } = *self.inner;
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
        if &*self.inner.path == "*" {
            if self.inner.method != Method::Options {
                return Err(Error::InvalidHead);
            }
        } else if self.inner.method == Method::Connect {
            // authority-form; path was normalized to `/` in parse_head — nothing to validate here.
        } else if self.inner.path.starts_with('/') {
            if self.inner.path.contains(['#', '\\']) {
                return Err(Error::InvalidHead);
            }
        } else {
            return Err(Error::InvalidHead);
        }

        if self.inner.method != Method::Connect
            && let Some(authority) = &self.inner.authority
            && let Some(host) = self.inner.request_headers.get_str(KnownHeaderName::Host)
            && !authority_matches_host(authority, host, self.inner.scheme.as_deref())
        {
            return Err(Error::InvalidHeaderValue(KnownHeaderName::Host.into()));
        }

        Ok(())
    }
}

/// The parsed components of an HTTP/1.x request-line (`method SP request-target SP HTTP-version`).
struct RequestLine {
    method: Method,
    path: Cow<'static, str>,
    authority: Option<Cow<'static, str>>,
    scheme: Option<Cow<'static, str>>,
    version: Version,
    /// The first component-level violation other than an unrecognized method, if any. A `Some`
    /// is carried up so the caller answers with a status derived from the error rather than
    /// closing.
    error: Option<Error>,
    /// A well-formed method token that isn't one we recognize. Kept apart from `error` because it
    /// is only reported when nothing else in the head is malformed: a request that is broken in
    /// any other way answers 400 even when its method is also unrecognized.
    unrecognized_method: Option<String>,
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
        let mut unrecognized_method = None;

        let method_bytes = &first_line[..first_space];
        let method = match Method::parse(method_bytes) {
            Ok(method) => method,
            Err(Error::UnrecognizedMethod(token)) => {
                unrecognized_method = Some(token);
                Method::Get
            }
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
            unrecognized_method,
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
            unrecognized_method: None,
        }
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

/// The status for a request head that outgrew `head_max_len`, by the part that was still
/// incomplete: header fields get 431, a request-target 414, and the method or anything else in the
/// request line 400.
fn oversized_head_status(head: &[u8]) -> Status {
    if CRLF.find(head).is_some() {
        return Status::RequestHeaderFieldsTooLarge;
    }
    let mut spaces = memchr::memchr_iter(b' ', head);
    match (spaces.next(), spaces.next()) {
        (Some(_), None) => Status::UriTooLong,
        _ => Status::BadRequest,
    }
}

/// Resolve the initial [`ReceivedBodyState`] for the incoming h1 request body from the parsed
/// headers. h1 requests without explicit framing default to an empty body — read-to-close on
/// inbound has no sender-side end-of-request signal.
fn initial_request_body_state(request_headers: &Headers) -> ReceivedBodyState {
    let chunked = request_headers.has_header(KnownHeaderName::TransferEncoding);
    let content_length = if chunked {
        None
    } else {
        request_headers.content_length().or(Some(0))
    };
    ReceivedBodyState::new_h1(content_length, chunked)
}
