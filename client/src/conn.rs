use crate::{pool::PoolEntry, util::encoding, Pool};
use encoding_rs::Encoding;
use futures_lite::{future::poll_once, io, AsyncReadExt, AsyncWriteExt, FutureExt};
use memchr::memmem::Finder;
use size::{Base, Size};
use std::{
    fmt::{self, Debug, Display, Formatter},
    future::{Future, IntoFuture},
    io::{ErrorKind, Write},
    ops::{Deref, DerefMut},
    pin::Pin,
    str::FromStr,
    time::Duration,
};
use trillium_http::{
    transport::BoxedTransport,
    Body, Error, HeaderName, HeaderValue, HeaderValues, Headers,
    KnownHeaderName::{Connection, ContentLength, Expect, Host, TransferEncoding},
    Method, ReceivedBody, ReceivedBodyState, Result, Status, Upgrade,
};
use trillium_server_common::{
    url::{Origin, Url},
    ArcedConnector, Connector, Transport,
};

const MAX_HEADERS: usize = 128;
const MAX_HEAD_LENGTH: usize = 2 * 1024;

/**
A wrapper error for [`trillium_http::Error`] or
[`serde_json::Error`]. Only available when the `json` crate feature is
enabled.
*/
#[cfg(feature = "json")]
#[derive(thiserror::Error, Debug)]
pub enum ClientSerdeError {
    /// A [`trillium_http::Error`]
    #[error(transparent)]
    HttpError(#[from] Error),

    /// A [`serde_json::Error`]
    #[error(transparent)]
    JsonError(#[from] serde_json::Error),
}

/**
a client connection, representing both an outbound http request and a
http response
*/
#[must_use]
pub struct Conn {
    pub(crate) url: Url,
    pub(crate) method: Method,
    pub(crate) request_headers: Headers,
    pub(crate) response_headers: Headers,
    pub(crate) transport: Option<BoxedTransport>,
    pub(crate) status: Option<Status>,
    pub(crate) request_body: Option<Body>,
    pub(crate) pool: Option<Pool<Origin, BoxedTransport>>,
    pub(crate) buffer: trillium_http::Buffer,
    pub(crate) response_body_state: ReceivedBodyState,
    pub(crate) config: ArcedConnector,
    pub(crate) headers_finalized: bool,
    pub(crate) timeout: Option<Duration>,
}

/// default http user-agent header
pub const USER_AGENT: &str = concat!("trillium-client/", env!("CARGO_PKG_VERSION"));

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
            .finish()
    }
}

impl Conn {
    /// borrow the request headers
    pub fn request_headers(&self) -> &Headers {
        &self.request_headers
    }

    /**
    chainable setter for [`inserting`](Headers::insert) a request header

    ```
    use trillium_testing::ClientConfig;


    let handler = |conn: trillium::Conn| async move {
        let header = conn.request_headers().get_str("some-request-header").unwrap_or_default();
        let response = format!("some-request-header was {}", header);
        conn.ok(response)
    };

    let client = trillium_client::Client::new(ClientConfig::new());

    trillium_testing::with_server(handler, |url| async move {
        let mut conn = client.get(url)
            .with_request_header("some-request-header", "header-value") // <--
            .await?;
        assert_eq!(
            conn.response_body().read_string().await?,
            "some-request-header was header-value"
        );
        Ok(())
    })
    ```
    */

    pub fn with_request_header(
        mut self,
        name: impl Into<HeaderName<'static>>,
        value: impl Into<HeaderValues>,
    ) -> Self {
        self.request_headers.insert(name, value);
        self
    }

    #[deprecated = "use Conn::with_request_header"]
    /// see [`with_request_header]
    pub fn with_header(
        self,
        name: impl Into<HeaderName<'static>>,
        value: impl Into<HeaderValues>,
    ) -> Self {
        self.with_request_header(name, value)
    }

    /**
    chainable setter for `extending` request headers

    ```
    let handler = |conn: trillium::Conn| async move {
        let header = conn.request_headers().get_str("some-request-header").unwrap_or_default();
        let response = format!("some-request-header was {}", header);
        conn.ok(response)
    };

    use trillium_testing::ClientConfig;
    let client = trillium_client::client(ClientConfig::new());

    trillium_testing::with_server(handler, move |url| async move {
        let mut conn = client.get(url)
            .with_request_headers([ // <--
                ("some-request-header", "header-value"),
                ("some-other-req-header", "other-header-value")
            ])
            .await?;
        assert_eq!(
            conn.response_body().read_string().await?,
            "some-request-header was header-value"
        );
        Ok(())
    })
    ```
    */
    pub fn with_request_headers<HN, HV, I>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = (HN, HV)> + Send,
        HN: Into<HeaderName<'static>>,
        HV: Into<HeaderValues>,
    {
        self.request_headers.extend(headers);
        self
    }

    /// see [`with_request_headers`]
    #[deprecated = "use Conn::with_request_headers"]
    pub fn with_headers<HN, HV, I>(self, headers: I) -> Self
    where
        I: IntoIterator<Item = (HN, HV)> + Send,
        HN: Into<HeaderName<'static>>,
        HV: Into<HeaderValues>,
    {
        self.with_request_headers(headers)
    }

    /// Chainable method to remove a request header if present
    pub fn without_request_header(mut self, name: impl Into<HeaderName<'static>>) -> Self {
        self.request_headers.remove(name);
        self
    }

    /// see [`without_request_header`]
    #[deprecated = "use Conn::without_request_header"]
    pub fn without_header(self, name: impl Into<HeaderName<'static>>) -> Self {
        self.without_request_header(name)
    }

    /**
    ```
    let handler = |conn: trillium::Conn| async move {
        conn.with_response_header("some-header", "some-value")
            .with_status(200)
    };

    use trillium_client::Client;
    use trillium_testing::ClientConfig;

    trillium_testing::with_server(handler, move |url| async move {
        let client = Client::new(ClientConfig::new());
        let conn = client.get(url).await?;

        let headers = conn.response_headers(); //<-

        assert_eq!(headers.get_str("some-header"), Some("some-value"));
        Ok(())
    })
    ```
    */
    pub fn response_headers(&self) -> &Headers {
        &self.response_headers
    }

    /**
    retrieves a mutable borrow of the request headers, suitable for
    appending a header. generally, prefer using chainable methods on
    Conn

    ```
    use trillium_testing::ClientConfig;
    use trillium_client::Client;

    let handler = |conn: trillium::Conn| async move {
        let header = conn.request_headers().get_str("some-request-header").unwrap_or_default();
        let response = format!("some-request-header was {}", header);
        conn.ok(response)
    };

    let client = Client::new(ClientConfig::new());

    trillium_testing::with_server(handler, move |url| async move {
        let mut conn = client.get(url);

        conn.request_headers_mut() //<-
            .insert("some-request-header", "header-value");

        let mut conn = conn.await?;

        assert_eq!(
            conn.response_body().read_string().await?,
            "some-request-header was header-value"
        );
        Ok(())
    })
    ```
    */
    pub fn request_headers_mut(&mut self) -> &mut Headers {
        &mut self.request_headers
    }

    /// get a mutable borrow of the response headers
    pub fn response_headers_mut(&mut self) -> &mut Headers {
        &mut self.response_headers
    }

    /**
    sets the request body on a mutable reference. prefer the chainable
    [`Conn::with_body`] wherever possible

    ```
    env_logger::init();
    use trillium_client::Client;
    use trillium_testing::ClientConfig;


    let handler = |mut conn: trillium::Conn| async move {
        let body = conn.request_body_string().await.unwrap();
        conn.ok(format!("request body was: {}", body))
    };

    trillium_testing::with_server(handler, move |url| async move {
        let client = Client::new(ClientConfig::new());
        let mut conn = client.post(url);

        conn.set_request_body("body"); //<-

        (&mut conn).await?;

        assert_eq!(conn.response_body().read_string().await?, "request body was: body");
        Ok(())
    });
    ```
     */
    pub fn set_request_body(&mut self, body: impl Into<Body>) {
        self.request_body = Some(body.into());
    }

    /**
    chainable setter for the request body

    ```
    env_logger::init();
    use trillium_testing::ClientConfig;
    use trillium_client::Client;

    let handler = |mut conn: trillium::Conn| async move {
        let body = conn.request_body_string().await.unwrap();
        conn.ok(format!("request body was: {}", body))
    };


    trillium_testing::with_server(handler, |url| async move {
        let client = Client::from(ClientConfig::default());
        let mut conn = client.post(url)
            .with_body("body") //<-
            .await?;

        assert_eq!(
            conn.response_body().read_string().await?,
            "request body was: body"
        );
        Ok(())
    });
    ```
     */
    pub fn with_body(mut self, body: impl Into<Body>) -> Self {
        self.set_request_body(body);
        self
    }

    /**
    chainable setter for json body. this requires the `json` crate feature to be enabled.
     */
    #[cfg(feature = "json")]
    pub fn with_json_body(self, body: &impl serde::Serialize) -> serde_json::Result<Self> {
        use trillium_http::KnownHeaderName;

        Ok(self
            .with_body(serde_json::to_string(body)?)
            .with_request_header(KnownHeaderName::ContentType, "application/json"))
    }

    pub(crate) fn response_encoding(&self) -> &'static Encoding {
        encoding(&self.response_headers)
    }

    /**
    retrieves the url for this conn.
    ```
    use trillium_testing::ClientConfig;
    use trillium_client::Client;
    let client = Client::from(ClientConfig::new());
    let conn = client.get("http://localhost:9080");

    let url = conn.url(); //<-

    assert_eq!(url.host_str().unwrap(), "localhost");
    ```
     */
    pub fn url(&self) -> &Url {
        &self.url
    }

    /**
    retrieves the url for this conn.
    ```
    use trillium_testing::ClientConfig;
    use trillium_client::Client;

    use trillium_testing::prelude::*;

    let client = Client::from(ClientConfig::new());
    let conn = client.get("http://localhost:9080");

    let method = conn.method(); //<-

    assert_eq!(method, Method::Get);
    ```
     */
    pub fn method(&self) -> Method {
        self.method
    }

    /**
    returns a [`ReceivedBody`] that borrows the connection inside this conn.
    ```
    env_logger::init();
    use trillium_testing::ClientConfig;
    use trillium_client::Client;



    let handler = |mut conn: trillium::Conn| async move {
        conn.ok("hello from trillium")
    };

    trillium_testing::with_server(handler, |url| async move {
        let client = Client::from(ClientConfig::new());
        let mut conn = client.get(url).await?;

        let response_body = conn.response_body(); //<-

        assert_eq!(19, response_body.content_length().unwrap());
        let string = response_body.read_string().await?;
        assert_eq!("hello from trillium", string);
        Ok(())
    });
    ```
     */

    #[allow(clippy::needless_borrow, clippy::needless_borrows_for_generic_args)]
    pub fn response_body(&mut self) -> ReceivedBody<'_, BoxedTransport> {
        ReceivedBody::new(
            self.response_content_length(),
            &mut self.buffer,
            self.transport.as_mut().unwrap(),
            &mut self.response_body_state,
            None,
            encoding(&self.response_headers),
        )
    }

    /**
    Attempt to deserialize the response body. Note that this consumes the body content.
     */
    #[cfg(feature = "json")]
    pub async fn response_json<T>(&mut self) -> std::result::Result<T, ClientSerdeError>
    where
        T: serde::de::DeserializeOwned,
    {
        let body = self.response_body().read_string().await?;
        Ok(serde_json::from_str(&body)?)
    }

    pub(crate) fn response_content_length(&self) -> Option<u64> {
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

    /**
    returns the status code for this conn. if the conn has not yet
    been sent, this will be None.

    ```
    use trillium_testing::ClientConfig;
    use trillium_client::Client;
    use trillium_testing::prelude::*;

    async fn handler(conn: trillium::Conn) -> trillium::Conn {
        conn.with_status(418)
    }

    trillium_testing::with_server(handler, |url| async move {
        let client = Client::new(ClientConfig::new());
        let conn = client.get(url).await?;
        assert_eq!(Status::ImATeapot, conn.status().unwrap());
        Ok(())
    });
    ```
     */
    pub fn status(&self) -> Option<Status> {
        self.status
    }

    /**
    Returns the conn or an [`UnexpectedStatusError`] that contains the conn

    ```
    use trillium_testing::ClientConfig;

    trillium_testing::with_server(trillium::Status::NotFound, |url| async move {
        let client = trillium_client::Client::new(ClientConfig::new());
        assert_eq!(
            client.get(url).await?.success().unwrap_err().to_string(),
            "expected a success (2xx) status code, but got 404 Not Found"
        );
        Ok(())
    });

    trillium_testing::with_server(trillium::Status::Ok, |url| async move {
        let client = trillium_client::Client::new(ClientConfig::new());
        assert!(client.get(url).await?.success().is_ok());
        Ok(())
    });
    ```
     */
    pub fn success(self) -> std::result::Result<Self, UnexpectedStatusError> {
        match self.status() {
            Some(status) if status.is_success() => Ok(self),
            _ => Err(self.into()),
        }
    }

    /**
    Returns this conn to the connection pool if it is keepalive, and
    closes it otherwise. This will happen asynchronously as a spawned
    task when the conn is dropped, but calling it explicitly allows
    you to block on it and control where it happens.
    */
    pub async fn recycle(mut self) {
        if self.is_keep_alive() && self.transport.is_some() && self.pool.is_some() {
            self.finish_reading_body().await;
        }
    }

    /// attempts to retrieve the connected peer address
    pub fn peer_addr(&self) -> Option<std::net::SocketAddr> {
        self.transport
            .as_ref()
            .and_then(|t| t.peer_addr().ok().flatten())
    }

    /// set the timeout for this conn
    ///
    /// this can also be set on the client with [`Client::set_timeout`] and [`Client::with_timeout`]
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = Some(timeout);
    }

    /// set the timeout for this conn
    ///
    /// this can also be set on the client with [`Client::set_timeout`] and [`Client::with_timeout`]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.set_timeout(timeout);
        self
    }

    // --- everything below here is private ---

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
                self.request_headers.insert(ContentLength, len.to_string());
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

            if len >= MAX_HEAD_LENGTH {
                return Err(Error::HeadersTooLong);
            }
        }
    }

    async fn parse_head(&mut self) -> Result<()> {
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

        self.response_headers.reserve(httparse_res.headers.len());
        for header in httparse_res.headers {
            let header_name = HeaderName::from_str(header.name)?;
            let header_value = HeaderValue::from(header.value.to_owned());
            self.response_headers.append(header_name, header_value);
        }

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

    fn is_keep_alive(&self) -> bool {
        self.response_headers
            .eq_ignore_ascii_case(Connection, "keep-alive")
    }

    async fn finish_reading_body(&mut self) {
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
            log::trace!("response body has been read to completion, checking transport back into pool for {}", &peer_addr);
            pool.insert(origin, PoolEntry::new(transport, None));
        } else {
            let content_length = self.response_content_length();
            let buffer = std::mem::take(&mut self.buffer);
            let response_body_state = self.response_body_state;
            let encoding = encoding(&self.response_headers);
            Connector::spawn(&self.config, async move {
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
    type Output = Result<Conn>;

    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'static>>;

    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move {
            if let Some(duration) = self.timeout {
                let config = self.config.clone();
                self.exec()
                    .or(async {
                        config.delay(duration).await;
                        Err(Error::TimedOut("Conn", duration))
                    })
                    .await?
            } else {
                self.exec().await?;
            }
            Ok(self)
        })
    }
}

impl<'conn> IntoFuture for &'conn mut Conn {
    type Output = Result<()>;

    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'conn>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            self.exec().await?;
            Ok(())
        })
    }
}

/// An unexpected http status code was received. Transform this back
/// into the conn with [`From::from`]/[`Into::into`].
///
/// Currently only returned by [`Conn::success`]
#[derive(Debug)]
pub struct UnexpectedStatusError(Box<Conn>);
impl From<Conn> for UnexpectedStatusError {
    fn from(value: Conn) -> Self {
        Self(Box::new(value))
    }
}

impl From<UnexpectedStatusError> for Conn {
    fn from(value: UnexpectedStatusError) -> Self {
        *value.0
    }
}

impl Deref for UnexpectedStatusError {
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl DerefMut for UnexpectedStatusError {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl std::error::Error for UnexpectedStatusError {}
impl Display for UnexpectedStatusError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.status() {
            Some(status) => f.write_fmt(format_args!(
                "expected a success (2xx) status code, but got {status}"
            )),
            None => f.write_str("expected a status code to be set, but none was"),
        }
    }
}
