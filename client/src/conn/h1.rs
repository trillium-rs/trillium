use super::{Conn, H2Pooled};
use crate::pool::{Acquire, PrimerGuard};
use futures_lite::{AsyncReadExt, AsyncWriteExt, future::poll_once, io};
use memchr::memmem::Finder;
use std::io::Write;
use trillium_http::{
    BufWriter, Error, Headers,
    KnownHeaderName::{Connection, ContentLength, Expect, Host, TransferEncoding},
    Method, ReceivedBodyState, Result, Status, Version,
};
use trillium_server_common::{Connector, Transport, url::Origin};

impl Conn {
    pub(super) fn finalize_headers_h1(&mut self) -> Result<()> {
        if self.headers_finalized {
            return Ok(());
        }

        let host = self.url.host_str().ok_or(Error::UnexpectedUriFormat)?;

        self.request_headers.try_insert_with(Host, || {
            self.url
                .port()
                .map_or_else(|| host.to_string(), |port| format!("{host}:{port}"))
        });

        if self.client.pool().is_none() {
            self.request_headers.try_insert(Connection, "close");
        }

        if self.upgrade {
            if self.request_body.is_some() {
                // A prelude body is sent as an open chunked stream (see `Body::keep_open`),
                // continuing into the post-handoff `Upgrade`. Force chunked framing and drop
                // any Content-Length — the stream is length-indeterminate.
                self.request_headers.remove(ContentLength);
                self.request_headers.insert(TransferEncoding, "chunked");
            } else if !self.request_headers.has_header(ContentLength) {
                // No prelude body: default the post-handoff stream to chunked framing so
                // the server keeps reading past the response head.
                self.request_headers.try_insert(TransferEncoding, "chunked");
            }
        } else {
            match self.body_len() {
                Some(0) => {}
                Some(len) => {
                    if self.http_version() >= Version::Http1_1 {
                        self.request_headers.insert(Expect, "100-continue");
                    }
                    self.request_headers.insert(ContentLength, len);
                }
                None => {
                    if self.http_version() >= Version::Http1_1 {
                        self.request_headers
                            .insert(Expect, "100-continue")
                            .insert(TransferEncoding, "chunked");
                    }
                    // HTTP/1.0: no chunked encoding; raw bytes are sent and connection close
                    // signals end-of-body
                }
            }
        }

        self.headers_finalized = true;
        Ok(())
    }

    async fn find_pool_candidate(&self, head: &[u8]) -> Result<Option<Box<dyn Transport>>> {
        let mut byte = [0];
        if let Some(pool) = self.client.pool() {
            for mut candidate in pool.candidates(&self.url.origin()) {
                if poll_once(candidate.read(&mut byte)).await.is_none()
                    && candidate.write_all(head).await.is_ok()
                {
                    return Ok(Some(candidate));
                }
            }
        }
        Ok(None)
    }

    async fn build_head(&mut self) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(128);
        let url = &self.url;
        let method = self.method;
        write!(buf, "{method} ")?;

        if let Some(target) = &self.request_target
            && matches!(method, Method::Connect | Method::Options)
        {
            write!(buf, "{target}")?;
        } else if method == Method::Connect {
            let host = url.host_str().ok_or(Error::UnexpectedUriFormat)?;

            let port = url
                .port_or_known_default()
                .ok_or(Error::UnexpectedUriFormat)?;

            write!(buf, "{host}:{port}")?;
        } else {
            write!(buf, "{}", url.path())?;
            if let Some(query) = url.query() {
                write!(buf, "?{query}")?;
            }
        }

        write!(buf, " {}\r\n", self.http_version())?;

        for (name, values) in &self.request_headers {
            if !name.is_valid() {
                return Err(Error::InvalidHeaderName);
            }

            for value in values {
                if !value.is_valid() {
                    return Err(Error::InvalidHeaderValue(name.to_owned()));
                }
                write!(buf, "{name}: ")?;
                buf.extend_from_slice(value.as_ref());
                write!(buf, "\r\n")?;
            }
        }

        write!(buf, "\r\n")?;
        log::trace!(
            "{}",
            std::str::from_utf8(&buf).unwrap().replace("\r\n", "\r\n> ")
        );

        Ok(buf)
    }

    async fn read_head(&mut self) -> Result<usize> {
        let Self {
            buffer,
            transport: Some(transport),
            ..
        } = self
        else {
            return Err(Error::Closed);
        };

        let mut len = buffer.len();
        let mut search_start = 0;
        let finder = Finder::new(b"\r\n\r\n");

        if len > 0 {
            if let Some(index) = finder.find(buffer) {
                return Ok(index + 4);
            }
            search_start = len.saturating_sub(3);
        }

        loop {
            buffer.expand();
            let bytes = transport.read(&mut buffer[len..]).await?;
            len += bytes;

            let search = finder.find(&buffer[search_start..len]);

            if let Some(index) = search {
                buffer.truncate(len);
                return Ok(search_start + index + 4);
            }

            search_start = len.saturating_sub(3);

            if bytes == 0 {
                if len == 0 {
                    return Err(Error::Closed);
                } else {
                    return Err(Error::InvalidHead);
                }
            }

            if len >= self.max_head_length {
                return Err(Error::HeadersTooLong);
            }
        }
    }

    async fn parse_head(&mut self) -> Result<()> {
        use std::str;

        let head_offset = self.read_head().await?;

        let space = memchr::memchr(b' ', &self.buffer[..head_offset]).ok_or(Error::InvalidHead)?;
        self.http_version = Some(
            str::from_utf8(&self.buffer[..space])
                .map_err(|_| Error::InvalidHead)?
                .parse()
                .map_err(|_| Error::InvalidHead)?,
        );
        self.status = Some(str::from_utf8(&self.buffer[space + 1..space + 4])?.parse()?);
        // The status-code is exactly three digits; the next octet must terminate it —
        // a SP before the reason-phrase, or the CR/LF ending the status-line. Reject a 4th digit so
        // e.g. `2000` isn't silently truncated to `200`.
        if !matches!(self.buffer.get(space + 4), Some(b' ' | b'\r' | b'\n')) {
            return Err(Error::InvalidHead);
        }
        let end_of_first_line = 2 + Finder::new("\r\n")
            .find(&self.buffer[..head_offset])
            .ok_or(Error::InvalidHead)?;

        // The network response head is authoritative: replace (not extend) any synthetic response
        // headers a handler may have set — e.g. a `Content-Length` from `set_response_body` — so
        // they can't merge with the wire headers into duplicates. Interim responses are cleared the
        // same way in `reset_interim_response_state`.
        self.response_headers = Headers::parse(&self.buffer[end_of_first_line..head_offset])
            .map_err(|_| Error::InvalidHead)?;

        self.buffer.ignore_front(head_offset);

        self.validate_response_headers()?;
        Ok(())
    }

    async fn send_body_and_parse_head(&mut self) -> Result<()> {
        // The upgrade path needs no special case here: `finalize_headers_h1` never sets
        // `Expect: 100-continue` for an upgrade, so the 100-continue block is skipped, and
        // `send_body` leaves the stream open (via `Body::keep_open`) instead of terminating
        // it. The caller continues writing through `Upgrade` after consuming the conn.
        if self
            .request_headers
            .eq_ignore_ascii_case(Expect, "100-continue")
        {
            log::trace!("Expecting 100-continue");
            loop {
                self.parse_head().await?;
                match self.status {
                    Some(Status::Continue) => {
                        self.reset_interim_response_state();
                        log::trace!("Received 100-continue, sending request body");
                        break;
                    }
                    Some(other) if is_interim(other) => {
                        log::trace!(
                            "Received interim response {other} while awaiting 100-continue, \
                             continuing to wait"
                        );
                        self.reset_interim_response_state();
                    }
                    _ => {
                        self.request_body.take();
                        log::trace!(
                            "Received a status code other than 100-continue, not sending request \
                             body"
                        );
                        self.response_body_state = self.initial_response_body_state();
                        return Ok(());
                    }
                }
            }
        }

        self.send_body().await?;
        loop {
            self.parse_head().await?;
            match self.status {
                Some(other) if is_interim(other) => {
                    log::trace!("Received interim response {other}, continuing to read");
                    self.reset_interim_response_state();
                }
                _ => break,
            }
        }

        self.response_body_state = self.initial_response_body_state();
        Ok(())
    }

    fn reset_interim_response_state(&mut self) {
        // Interim responses must not contribute headers to the final response, so clear them
        // before reading the next head.
        self.status = None;
        self.response_headers = Headers::new();
    }

    async fn send_body(&mut self) -> Result<()> {
        let Some(mut body) = self.request_body.take() else {
            return Ok(());
        };

        let upgrade = self.upgrade;
        if upgrade {
            // Leave the chunked stream unterminated; the `Upgrade` owns the terminator.
            body = body.keep_open();
        }

        // HTTP/1.0 doesn't support chunked transfer encoding. Stream raw bytes directly;
        // connection close signals end-of-body to the server.
        if self.http_version() < Version::Http1_1 && body.len().is_none() {
            let transport = self.transport.as_mut().ok_or(Error::Closed)?;
            io::copy(&mut body.into_reader(), transport).await?;
            return Ok(());
        }

        let copy_loops_per_yield = self.context.config().copy_loops_per_yield();
        let Self {
            transport,
            request_trailers,
            ..
        } = self;

        let transport = transport.as_mut().ok_or(Error::Closed)?;

        let max_buf = self.context.config().response_buffer_max_len();
        let mut bufwriter = BufWriter::new_with_buffer(
            Vec::with_capacity(self.context.config().response_buffer_len()),
            transport,
            max_buf,
        );

        bufwriter.copy_from(&mut body, copy_loops_per_yield).await?;

        // When an upgrade follows, the `Upgrade` owns the terminator; the body's trailers
        // (if any) ride onto it and merge with whatever the caller emits. Skip the
        // trailer-section + terminating CRLF here.
        if !upgrade {
            *request_trailers = body.trailers();
            if let Some(trailers) = &*request_trailers {
                let buf = bufwriter.buffer_mut();
                for (name, values) in trailers {
                    if !name.is_valid() {
                        return Err(Error::InvalidHeaderName);
                    }

                    for value in values {
                        if !value.is_valid() {
                            return Err(Error::InvalidHeaderValue(name.to_owned()));
                        }
                        write!(buf, "{name}: ")?;
                        buf.extend_from_slice(value.as_ref());
                        write!(buf, "\r\n")?;
                    }
                }

                log::trace!("sending request trailers: {trailers:?}");
            }

            if body.len().is_none() {
                write!(bufwriter.buffer_mut(), "\r\n")?;
            }
        }

        bufwriter.flush().await?;
        Ok(())
    }

    fn validate_response_headers(&self) -> Result<()> {
        // `chunked` is the only transfer-coding trillium decodes, so the only Transfer-Encoding we
        // can frame unambiguously is a single `chunked`. Multiple header lines coalesce into one
        // ordered coding list, so we flatten across lines and commas, then require exactly one
        // coding equal to `chunked`. Anything else — a repeated `chunked`, `chunked` not final,
        // another/unknown coding, or an empty value — is rejected rather than decoded-once
        // or read-to-close: those framing fallbacks are response-smuggling vectors. Matches
        // the server's request-side rule so both halves of a proxy frame identically.
        let chunked = if self.response_headers.has_header(TransferEncoding) {
            let mut codings = self.response_headers.token_iter(TransferEncoding);
            match (codings.next(), codings.next()) {
                (Some(only), None) if only.eq_ignore_ascii_case("chunked") => true,
                _ => return Err(Error::UnexpectedHeader(TransferEncoding.into())),
            }
        } else {
            false
        };

        let content_length = self.response_headers.get_values(ContentLength);

        if chunked && content_length.is_some() {
            return Err(Error::UnexpectedHeader(ContentLength.into()));
        }

        // A malformed or duplicated Content-Length must be rejected, not coerced to read-to-close
        // (as `get_str(..).parse().ok()` silently would) — that's a response-smuggling vector via
        // trillium-proxy. Shared with the server request parser so both reject identically.
        trillium_http::validate_content_length(content_length)?;
        Ok(())
    }

    /// Whether the underlying transport will be kept alive and pooled for h1 reuse after this
    /// response's body is consumed — the keep-alive decision the recycle path acts on. Always
    /// `false` for h2/h3: those connections are reused through their own multiplexing pools, and
    /// the transport on the conn is a spent single-use stream rather than a poolable connection.
    /// Exposed (hidden) for the response-parser corpus harness.
    ///
    /// For h1: HTTP/1.1 is persistent unless a `Connection: close` appears on either side;
    /// HTTP/1.0 is non-persistent unless both sides send `Connection: keep-alive`.
    #[doc(hidden)]
    pub fn is_keep_alive(&self) -> bool {
        // Keep-alive is an HTTP/1.x connection-persistence concept. An h2/h3 conn's transport is a
        // single-use stream and connection reuse lives in the h2/h3 pools, so it is never
        // h1-poolable — without this guard the `!= Http1_0` fallback below would wrongly say it is.
        if self.http_version() > Version::Http1_1 {
            return false;
        }

        let has_token = |headers: &Headers, token: &str| {
            headers
                .token_iter(Connection)
                .any(|t| t.eq_ignore_ascii_case(token))
        };

        if has_token(&self.request_headers, "close") || has_token(&self.response_headers, "close") {
            false
        } else if has_token(&self.request_headers, "keep-alive")
            && has_token(&self.response_headers, "keep-alive")
        {
            true
        } else {
            self.http_version() != Version::Http1_0
        }
    }

    pub(super) fn response_content_length(&self) -> Option<u64> {
        if self.status == Some(Status::NoContent)
            || self.status == Some(Status::NotModified)
            || self.method == Method::Head
        {
            Some(0)
        } else {
            self.response_headers.content_length()
        }
    }

    /// Resolve the initial [`ReceivedBodyState`] for the inbound response body from
    /// the parsed status, method, and response headers. HEAD requests and 204/304
    /// responses produce [`ReceivedBodyState::End`] regardless of headers; chunked
    /// transfer-encoding produces [`ReceivedBodyState::Chunked`]; everything else —
    /// including responses without explicit framing, which become read-to-close —
    /// produces [`ReceivedBodyState::Raw`].
    fn initial_response_body_state(&self) -> ReceivedBodyState {
        if self.status == Some(Status::NoContent)
            || self.status == Some(Status::NotModified)
            || self.method == Method::Head
        {
            return ReceivedBodyState::End;
        }
        // `validate_response_headers` (run during `parse_head`) has already established that a
        // Transfer-Encoding, if present, is exactly a single `chunked` and never coexists with
        // Content-Length — so its mere presence means chunked framing.
        let chunked = self.response_headers.has_header(TransferEncoding);
        let content_length = self.response_headers.content_length();
        ReceivedBodyState::new_h1(content_length, chunked)
    }

    pub(super) async fn exec_h1_or_promote_h2(&mut self) -> Result<()> {
        // An h3 hint reaches here only when no h3 client is configured to honor it; resume
        // auto-discovery (h1 / ALPN-promoted h2) rather than pinning, matching the
        // h3-connect-failure fallback. An explicit h1.1 / h1.0 pin is left intact.
        if matches!(self.http_version, Some(v) if v > Version::Http1_1) {
            self.http_version = None;
        }

        self.finalize_headers_h1()?;
        let head = self.build_head().await?;

        // An idle keepalive transport for this origin short-circuits the connect entirely.
        if let Some(transport) = self.find_pool_candidate(&head).await? {
            log::debug!("reusing connection to {:?}", transport.peer_addr()?);
            return self.exec_h1_on_transport(transport).await;
        }

        // Cold connect. With no version pin, coalesce concurrent cold-starts to this origin through
        // the h2 pool's in-flight slot: when the connection turns out to be multiplexed (ALPN
        // `h2`), a burst opens one connection and the waiters share it rather than each opening
        // their own. An explicit h1 pin skips this — it neither shares an h2 connection nor
        // promotes one — and its connection advertises only `http/1.1` (see `origin_destination`).
        let h2_pool = if self.http_version.is_none() {
            self.client.h2_pool().cloned()
        } else {
            None
        };
        if let Some(h2_pool) = h2_pool {
            match h2_pool.acquire(self.url.origin(), |p| p.classify()) {
                Acquire::Ready(pooled) => {
                    return self
                        .exec_h2_on_connection(pooled.connection().clone())
                        .await;
                }
                Acquire::Await(cell) => {
                    if let Some(pooled) = cell.wait().await {
                        return self
                            .exec_h2_on_connection(pooled.connection().clone())
                            .await;
                    }
                    // The primer produced no shareable connection (it went h1, or its connect
                    // failed); connect for ourselves below.
                }
                Acquire::Primer(guard) => {
                    return self.connect_and_dispatch(head, Some(guard)).await;
                }
            }
        }

        self.connect_and_dispatch(head, None).await
    }

    /// Run the request over an h1 transport (pooled or freshly connected), then learn any
    /// h3 endpoint the response advertises via `Alt-Svc`.
    async fn exec_h1_on_transport(&mut self, transport: Box<dyn Transport>) -> Result<()> {
        self.transport = Some(transport);
        self.send_body_and_parse_head().await?;
        if let Some(h3) = self.client.h3() {
            self.update_alt_svc_from_response(h3);
        }
        Ok(())
    }

    /// Open a fresh connection and dispatch the request: promote to h2 when ALPN negotiates it,
    /// otherwise send over h1.
    ///
    /// `guard` is `Some` when we are the elected primer for this origin's in-flight slot —
    /// resolving it (or dropping it on a connect error) releases any waiters. It is `None` when
    /// pooling is disabled, or when we are a waiter that fell through to its own connect.
    async fn connect_and_dispatch(
        &mut self,
        head: Vec<u8>,
        guard: Option<PrimerGuard<Origin, H2Pooled>>,
    ) -> Result<()> {
        // On a connect error, `?` returns and `guard` drops here, waking any waiters to connect
        // for themselves rather than hang.
        let destination = self.origin_destination().await?;
        let mut transport = self.client.connector().connect_to(destination).await?;
        log::debug!("opened new connection to {:?}", transport.peer_addr()?);

        // Promote to h2 only when auto-discovering: an explicit h1 pin advertised only `http/1.1`
        // (see `origin_destination`), so a peer that nonetheless reports `h2` here is honored as an
        // h1 connection rather than overriding the pin.
        if self.http_version.is_none()
            && self.client.h2_pool().is_some()
            && transport.negotiated_alpn().as_deref() == Some(b"h2")
        {
            return self.promote_and_exec_h2(transport, guard).await;
        }

        // Not multiplexed: release any waiters to connect for themselves, then send over h1.
        if let Some(guard) = guard {
            guard.resolve_absent();
        }
        transport.write_all(&head).await?;
        self.exec_h1_on_transport(transport).await
    }
}

/// All 1xx codes are interim *except* `101 Switching Protocols`, which is a final response
/// that hands the connection off to a different protocol.
fn is_interim(status: Status) -> bool {
    status.is_informational() && status != Status::SwitchingProtocols
}
