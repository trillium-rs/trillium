use crate::{
    BufWriter, Buffer, Conn, ConnectionStatus, Error, Headers, KnownHeaderName, Method,
    ReceivedBody, Result, ServerConfig, Status, TypeSet, Version, after_send::AfterSend,
    conn::ReceivedBodyState, copy, util::encoding,
};
use futures_lite::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use memchr::memmem::Finder;
use std::{
    borrow::Cow,
    sync::Arc,
    time::{Instant, SystemTime},
};

impl<Transport> Conn<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    pub(super) fn finalize_response_headers_1x(&mut self) {
        if self.status == Some(Status::SwitchingProtocols) {
            return;
        }

        self.response_headers
            .try_insert_with(KnownHeaderName::Date, || {
                httpdate::fmt_http_date(SystemTime::now())
            });

        if !matches!(self.status, Some(Status::NotModified | Status::NoContent)) {
            let has_content_length = if let Some(len) = self.body_len() {
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

        if self.server_config.swansong.state().is_shutting_down() {
            self.response_headers
                .insert(KnownHeaderName::Connection, "close");
        }
    }

    pub(crate) async fn send(mut self) -> Result<ConnectionStatus<Transport>> {
        let mut output_buffer =
            Vec::with_capacity(self.server_config.http_config.response_buffer_len);
        self.write_headers(&mut output_buffer)?;

        let mut bufwriter = BufWriter::new_with_buffer(output_buffer, &mut self.transport);

        if self.method != Method::Head
            && !matches!(self.status, Some(Status::NotModified | Status::NoContent))
            && let Some(body) = self.response_body.take()
        {
            copy(
                body,
                &mut bufwriter,
                self.server_config.http_config.copy_loops_per_yield,
            )
            .await?;
        }

        bufwriter.flush().await?;
        self.after_send.call(true.into());
        self.finish().await
    }

    pub(super) fn needs_100_continue(&self) -> bool {
        self.request_body_state == ReceivedBodyState::Start
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
            &self.server_config.http_config,
        )
    }

    fn validate_headers(request_headers: &Headers) -> Result<()> {
        if request_headers
            .get_values(KnownHeaderName::ContentLength)
            .is_some_and(|v| v.len() > 1)
        {
            return Err(Error::UnexpectedHeader(
                KnownHeaderName::ContentLength.into(),
            ));
        }

        if let Some(te) = request_headers.get_values(KnownHeaderName::TransferEncoding) {
            if !te
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case("chunked"))
            {
                return Err(Error::UnexpectedHeader(
                    KnownHeaderName::TransferEncoding.into(),
                ));
            }
        }

        if request_headers.has_header(KnownHeaderName::ContentLength)
            && request_headers.has_header(KnownHeaderName::TransferEncoding)
        {
            return Err(Error::UnexpectedHeader(
                KnownHeaderName::ContentLength.into(),
            ));
        }

        Ok(())
    }

    // /// # Create a new `Conn`
    // ///
    // /// This function creates a new conn from the provided
    // /// [`Transport`][crate::transport::Transport], as well as any
    // /// bytes that have already been read from the transport, and a
    // /// [`Swansong`] instance that will be used to signal graceful
    // /// shutdown.
    // ///
    // /// # Errors
    // ///
    // /// This will return an error variant if:
    // ///
    // /// * there is an io error when reading from the underlying transport
    // /// * headers are too long
    // /// * we are unable to parse some aspect of the request
    // /// * the request is an unsupported http version
    // /// * we cannot make sense of the headers, such as if there is a
    // /// `content-length` header as well as a `transfer-encoding: chunked`
    // /// header.
    // pub async fn new(transport: Transport, bytes: Vec<u8>, swansong: Swansong) -> Result<Self> {
    //     Self::new_internal(DEFAULT_CONFIG, transport, bytes.into(), swansong, None).await
    // }

    #[cfg(not(feature = "parse"))]
    pub(crate) async fn new_internal(
        server_config: Arc<ServerConfig>,
        mut transport: Transport,
        mut buffer: Buffer,
    ) -> Result<Self> {
        use crate::{HeaderName, HeaderValue};
        use httparse::{EMPTY_HEADER, Request};

        let (head_size, start_time) =
            Self::head(&mut transport, &mut buffer, &server_config).await?;

        let mut headers = vec![EMPTY_HEADER; server_config.http_config.max_headers];
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

        Self::validate_headers(&request_headers)?;

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

        let response_headers = server_config
            .shared_state()
            .get::<Headers>()
            .cloned()
            .unwrap_or_default();

        buffer.ignore_front(head_size);

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
            request_body_state: ReceivedBodyState::Start,
            secure: false,
            after_send: AfterSend::default(),
            start_time,
            peer_ip: None,
            server_config,
            authority,
            scheme: None,
            h3_connection: None,
            protocol: None,
        })
    }

    #[cfg(feature = "parse")]
    pub(crate) async fn new_internal(
        server_config: Arc<ServerConfig>,
        mut transport: Transport,
        mut buffer: Buffer,
    ) -> Result<Self> {
        let (head_size, start_time) =
            Self::head(&mut transport, &mut buffer, &server_config).await?;

        let first_line_index = Finder::new(b"\r\n")
            .find(&buffer)
            .ok_or(Error::InvalidHead)?;

        let mut spaces = memchr::memchr_iter(b' ', &buffer[..first_line_index]);
        let first_space = spaces.next().ok_or(Error::MissingMethod)?;
        let method = Method::parse(&buffer[0..first_space])?;
        let second_space = spaces.next().ok_or(Error::RequestPathMissing)?;
        let mut path: Cow<'static, str> = Cow::Owned(
            std::str::from_utf8(&buffer[first_space + 1..second_space])
                .map_err(|_| Error::RequestPathMissing)?
                .to_string(),
        );

        if path.is_empty() {
            return Err(Error::InvalidHead);
        }

        let version = Version::parse(&buffer[second_space + 1..first_line_index])?;
        if !matches!(version, Version::Http1_1 | Version::Http1_0) {
            return Err(Error::UnsupportedVersion(version));
        }

        let request_headers = Headers::parse(&buffer[first_line_index + 2..head_size])?;

        Self::validate_headers(&request_headers)?;

        let mut authority = None;

        if method == Method::Connect {
            authority = Some(path);
            path = Cow::Borrowed("/");
        }

        let response_headers = server_config
            .shared_state()
            .get::<Headers>()
            .cloned()
            .unwrap_or_default();

        buffer.ignore_front(head_size);

        Ok(Self {
            server_config,
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
            request_body_state: ReceivedBodyState::Start,
            secure: false,
            after_send: AfterSend::default(),
            start_time,
            peer_ip: None,
            authority,
            scheme: None,
            h3_connection: None,
            protocol: None,
        })
    }

    pub(super) async fn send_100_continue(&mut self) -> Result<()> {
        log::trace!("sending 100-continue");
        Ok(self
            .transport
            .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
            .await?)
    }

    async fn head(
        transport: &mut Transport,
        buf: &mut Buffer,
        server_config: &ServerConfig,
    ) -> Result<(usize, Instant)> {
        let mut len = 0;
        let mut start_with_read = buf.is_empty();
        let mut instant = None;
        let finder = Finder::new(b"\r\n\r\n");
        loop {
            if len >= server_config.http_config.head_max_len {
                return Err(Error::HeadersTooLong);
            }

            let bytes = if start_with_read {
                buf.expand();
                if len == 0 {
                    server_config
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

    async fn next(mut self) -> Result<Self> {
        if !self.needs_100_continue() || self.request_body_state != ReceivedBodyState::Start {
            self.build_request_body().drain().await?;
        }
        Conn::new_internal(self.server_config, self.transport, self.buffer).await
    }

    fn should_close(&self) -> bool {
        let has_token = |headers: &Headers, token: &str| {
            headers
                .get_lower(KnownHeaderName::Connection)
                .is_some_and(|v| v.split(',').any(|t| t.trim() == token))
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
            match self.next().await {
                Err(Error::Closed) => {
                    log::trace!("connection closed by client");
                    Ok(ConnectionStatus::Close)
                }
                Err(e) => Err(e),
                Ok(conn) => Ok(ConnectionStatus::Conn(conn)),
            }
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
        } else if self.version == Version::Http3 {
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
        use std::io::Write;
        let status = self.status().unwrap_or(Status::NotFound);

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
            &self.response_headers
        );

        let panic_on_invalid = self
            .server_config
            .http_config
            .panic_on_invalid_response_headers;

        for (name, values) in &self.response_headers {
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

        write!(output_buffer, "\r\n")?;
        Ok(())
    }
}
