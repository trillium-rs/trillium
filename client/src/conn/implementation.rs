use super::Conn;
use crate::{pool::PoolEntry, util::encoding};
use futures_lite::{AsyncReadExt, AsyncWriteExt, future::poll_once, io};
use memchr::memmem::Finder;
use size::{Base, Size};
use std::{
    fmt::{self, Debug, Formatter},
    future::{Future, IntoFuture},
    io::{ErrorKind, Write},
    pin::Pin,
};
use trillium_http::{
    Body, Error,
    KnownHeaderName::{Connection, ContentLength, Expect, Host, TransferEncoding},
    Method, ReceivedBody, ReceivedBodyState, Result, Status, TypeSet, Upgrade,
    transport::BoxedTransport,
};
use trillium_server_common::{Connector, Transport};

impl Conn {
    fn finalize_headers(&mut self) -> Result<()> {
        if self.headers_finalized {
            return Ok(());
        }

        let host = self.url.host_str().ok_or(Error::UnexpectedUriFormat)?;

        self.request_headers.try_insert_with(Host, || {
            self.url
                .port()
                .map_or_else(|| host.to_string(), |port| format!("{host}:{port}"))
        });

        if self.pool.is_none() {
            self.request_headers.try_insert(Connection, "close");
        }

        match self.body_len() {
            Some(0) => {}
            Some(len) => {
                self.request_headers.insert(Expect, "100-continue");
                self.request_headers.insert(ContentLength, len);
            }
            None => {
                self.request_headers.insert(Expect, "100-continue");
                self.request_headers.insert(TransferEncoding, "chunked");
            }
        }

        self.headers_finalized = true;
        Ok(())
    }

    fn body_len(&self) -> Option<u64> {
        if let Some(ref body) = self.request_body {
            body.len()
        } else {
            Some(0)
        }
    }

    async fn find_pool_candidate(&self, head: &[u8]) -> Result<Option<BoxedTransport>> {
        let mut byte = [0];
        if let Some(pool) = &self.pool {
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

    async fn connect_and_send_head(&mut self) -> Result<()> {
        if self.transport.is_some() {
            return Err(Error::Io(std::io::Error::new(
                ErrorKind::AlreadyExists,
                "conn already connected",
            )));
        }

        let head = self.build_head().await?;

        let transport = match self.find_pool_candidate(&head).await? {
            Some(transport) => {
                log::debug!("reusing connection to {:?}", transport.peer_addr()?);
                transport
            }

            None => {
                let mut transport = self.config.connect(&self.url).await?;
                log::debug!("opened new connection to {:?}", transport.peer_addr()?);
                transport.write_all(&head).await?;
                transport
            }
        };

        self.transport = Some(transport);
        Ok(())
    }

    async fn build_head(&mut self) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(128);
        let url = &self.url;
        let method = self.method;
        write!(buf, "{method} ")?;

        if method == Method::Connect {
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

        write!(buf, " HTTP/1.1\r\n")?;

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

    fn transport(&mut self) -> &mut BoxedTransport {
        self.transport.as_mut().unwrap()
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

    #[cfg(not(feature = "parse"))]
    async fn parse_head(&mut self) -> Result<()> {
        const MAX_HEADERS: usize = 128;
        use crate::{HeaderName, HeaderValue};
        use std::str::FromStr;

        let head_offset = self.read_head().await?;
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut httparse_res = httparse::Response::new(&mut headers);
        let parse_result =
            httparse_res
                .parse(&self.buffer[..head_offset])
                .map_err(|e| match e {
                    httparse::Error::HeaderName => Error::InvalidHeaderName,
                    httparse::Error::HeaderValue => Error::InvalidHeaderValue("unknown".into()),
                    httparse::Error::Status => Error::InvalidStatus,
                    httparse::Error::TooManyHeaders => Error::HeadersTooLong,
                    httparse::Error::Version => Error::InvalidVersion,
                    _ => Error::InvalidHead,
                })?;

        match parse_result {
            httparse::Status::Complete(n) if n == head_offset => {}
            _ => return Err(Error::InvalidHead),
        }

        self.status = httparse_res.code.map(|code| code.try_into().unwrap());

        for header in httparse_res.headers {
            let header_name = HeaderName::from_str(header.name)?;
            let header_value = HeaderValue::from(header.value.to_owned());
            self.response_headers.append(header_name, header_value);
        }

        self.buffer.ignore_front(head_offset);

        self.validate_response_headers()?;
        Ok(())
    }

    #[cfg(feature = "parse")]
    async fn parse_head(&mut self) -> Result<()> {
        use std::str;

        let head_offset = self.read_head().await?;

        let space = memchr::memchr(b' ', &self.buffer[..head_offset]).ok_or(Error::InvalidHead)?;
        self.http_version = str::from_utf8(&self.buffer[..space])
            .map_err(|_| Error::InvalidHead)?
            .parse()
            .map_err(|_| Error::InvalidHead)?;
        self.status = Some(str::from_utf8(&self.buffer[space + 1..space + 4])?.parse()?);
        let end_of_first_line = 2 + Finder::new("\r\n")
            .find(&self.buffer[..head_offset])
            .ok_or(Error::InvalidHead)?;

        self.response_headers
            .extend_parse(&self.buffer[end_of_first_line..head_offset])
            .map_err(|_| Error::InvalidHead)?;

        self.buffer.ignore_front(head_offset);

        self.validate_response_headers()?;
        Ok(())
    }

    async fn send_body_and_parse_head(&mut self) -> Result<()> {
        if self
            .request_headers
            .eq_ignore_ascii_case(Expect, "100-continue")
        {
            log::trace!("Expecting 100-continue");
            self.parse_head().await?;
            if self.status == Some(Status::Continue) {
                self.status = None;
                log::trace!("Received 100-continue, sending request body");
            } else {
                self.request_body.take();
                log::trace!(
                    "Received a status code other than 100-continue, not sending request body"
                );
                return Ok(());
            }
        }

        self.send_body().await?;
        loop {
            self.parse_head().await?;
            if self.status == Some(Status::Continue) {
                self.status = None;
            } else {
                break;
            }
        }

        Ok(())
    }

    async fn send_body(&mut self) -> Result<()> {
        if let Some(mut body) = self.request_body.take() {
            io::copy(&mut body, self.transport()).await?;
        }
        Ok(())
    }

    fn validate_response_headers(&self) -> Result<()> {
        let content_length = self.response_headers.has_header(ContentLength);

        let transfer_encoding_chunked = self
            .response_headers
            .eq_ignore_ascii_case(TransferEncoding, "chunked");

        if content_length && transfer_encoding_chunked {
            Err(Error::UnexpectedHeader(ContentLength.into()))
        } else {
            Ok(())
        }
    }

    pub(super) fn is_keep_alive(&self) -> bool {
        self.response_headers
            .eq_ignore_ascii_case(Connection, "keep-alive")
    }

    pub(super) async fn finish_reading_body(&mut self) {
        if self.response_body_state != ReceivedBodyState::End {
            let body = self.response_body();
            match body.drain().await {
                Ok(drain) => log::debug!(
                    "drained {}",
                    Size::from_bytes(drain).format().with_base(Base::Base10)
                ),
                Err(e) => log::warn!("failed to drain body, {:?}", e),
            }
        }
    }

    async fn exec(&mut self) -> Result<()> {
        self.finalize_headers()?;
        self.connect_and_send_head().await?;
        self.send_body_and_parse_head().await?;
        Ok(())
    }

    pub(super) fn response_content_length(&self) -> Option<u64> {
        if self.status == Some(Status::NoContent)
            || self.status == Some(Status::NotModified)
            || self.method == Method::Head
        {
            Some(0)
        } else {
            self.response_headers
                .get_str(ContentLength)
                .and_then(|c| c.parse().ok())
        }
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        if !self.is_keep_alive() {
            return;
        }

        let Some(transport) = self.transport.take() else {
            return;
        };
        let Ok(Some(peer_addr)) = transport.peer_addr() else {
            return;
        };
        let Some(pool) = self.pool.take() else { return };

        let origin = self.url.origin();

        if self.response_body_state == ReceivedBodyState::End {
            log::trace!(
                "response body has been read to completion, checking transport back into pool for \
                 {}",
                &peer_addr
            );
            pool.insert(origin, PoolEntry::new(transport, None));
        } else {
            let content_length = self.response_content_length();
            let buffer = std::mem::take(&mut self.buffer);
            let response_body_state = self.response_body_state;
            let encoding = encoding(&self.response_headers);
            self.config.runtime().spawn(async move {
                let mut response_body = ReceivedBody::new(
                    content_length,
                    buffer,
                    transport,
                    response_body_state,
                    None,
                    encoding,
                );

                match io::copy(&mut response_body, io::sink()).await {
                    Ok(bytes) => {
                        let transport = response_body.take_transport().unwrap();
                        log::trace!(
                            "read {} bytes in order to recycle conn for {}",
                            bytes,
                            &peer_addr
                        );
                        pool.insert(origin, PoolEntry::new(transport, None));
                    }

                    Err(ioerror) => log::error!("unable to recycle conn due to {}", ioerror),
                };
            });
        }
    }
}

impl From<Conn> for Body {
    fn from(conn: Conn) -> Body {
        let received_body: ReceivedBody<'static, _> = conn.into();
        received_body.into()
    }
}

impl From<Conn> for ReceivedBody<'static, BoxedTransport> {
    fn from(mut conn: Conn) -> Self {
        let _ = conn.finalize_headers();
        let origin = conn.url.origin();

        let on_completion =
            conn.pool
                .take()
                .map(|pool| -> Box<dyn Fn(BoxedTransport) + Send + Sync> {
                    Box::new(move |transport| {
                        pool.insert(origin.clone(), PoolEntry::new(transport, None));
                    })
                });

        ReceivedBody::new(
            conn.response_content_length(),
            std::mem::take(&mut conn.buffer),
            conn.transport.take().unwrap(),
            conn.response_body_state,
            on_completion,
            conn.response_encoding(),
        )
    }
}

impl From<Conn> for Upgrade<BoxedTransport> {
    fn from(mut conn: Conn) -> Self {
        Upgrade::new(
            std::mem::take(&mut conn.request_headers),
            conn.url.path().to_string(),
            conn.method,
            conn.transport.take().unwrap(),
            std::mem::take(&mut conn.buffer),
        )
    }
}

impl IntoFuture for Conn {
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'static>>;
    type Output = Result<Conn>;

    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move { (&mut self).await.map(|()| self) })
    }
}

impl<'conn> IntoFuture for &'conn mut Conn {
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'conn>>;
    type Output = Result<()>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(duration) = self.timeout {
                self.config
                    .runtime()
                    .timeout(duration, self.exec())
                    .await
                    .unwrap_or(Err(Error::TimedOut("Conn", duration)))?;
            } else {
                self.exec().await?;
            }
            Ok(())
        })
    }
}

impl Debug for Conn {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conn")
            .field("url", &self.url)
            .field("method", &self.method)
            .field("request_headers", &self.request_headers)
            .field("response_headers", &self.response_headers)
            .field("status", &self.status)
            .field("request_body", &self.request_body)
            .field("pool", &self.pool)
            .field("buffer", &String::from_utf8_lossy(&self.buffer))
            .field("response_body_state", &self.response_body_state)
            .field("config", &self.config)
            .field("state", &self.state)
            .finish()
    }
}

impl AsRef<TypeSet> for Conn {
    fn as_ref(&self) -> &TypeSet {
        &self.state
    }
}
impl AsMut<TypeSet> for Conn {
    fn as_mut(&mut self) -> &mut TypeSet {
        &mut self.state
    }
}
