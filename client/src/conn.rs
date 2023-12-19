use crate::{pool::PoolEntry, util::encoding, Pool};
use encoding_rs::Encoding;
use futures_lite::{future::poll_once, io, AsyncReadExt, AsyncWriteExt};
use memchr::memmem::Finder;
use std::{
    convert::TryInto,
    fmt::{self, Debug, Display, Formatter},
    future::{Future, IntoFuture},
    io::{ErrorKind, Write},
    ops::{Deref, DerefMut},
    pin::Pin,
    str::FromStr,
    sync::Arc,
};
use trillium_http::{
    transport::BoxedTransport,
    Body, Error, HeaderName, HeaderValue, HeaderValues, Headers,
    KnownHeaderName::{
        Connection, ContentLength, Expect, Host, ProxyConnection, TransferEncoding, UserAgent,
    },
    Method, ReceivedBody, ReceivedBodyState, Result, StateSet, Status, Stopper, Upgrade,
};
use trillium_server_common::{Connector, ObjectSafeConnector, Transport};
use url::{Origin, Url};

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
    url: Url,
    method: Method,
    request_headers: Headers,
    response_headers: Headers,
    transport: Option<BoxedTransport>,
    status: Option<Status>,
    request_body: Option<Body>,
    pool: Option<Pool<Origin, BoxedTransport>>,
    buffer: trillium_http::Buffer,
    response_body_state: ReceivedBodyState,
    config: Arc<dyn ObjectSafeConnector>,
    headers_finalized: bool,
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
    // * NOTICE TO READERS: *
    //
    // Conn::new is currently commented out in order to encourage
    // people to use a Client.  Aside from a single Arc::clone,
    // there is no performance advantage to directly constructing a
    // Conn, and aside from tests, rarely does an application make a
    // single standalone http request.
    //
    // Disadvantages of constructing a new Connector for each Conn
    // 1. tls connectors are relatively expensive to construct, but
    //    can be reused from within a Client
    // 2. it becomes harder to take advantage of connection pooling
    //    if at a later point you want to do so
    //
    // If this reasoning is not compelling to you, please open an
    // issue or discussion -- this comment exists because I'm not
    // certain.
    //
    // /**
    // ```
    // use trillium_testing::prelude::*;
    //
    // let conn = Conn::new("get", "http://trillium.rs", ClientConfig::default()); //<-
    // assert_eq!(conn.method(), Method::Get);
    // assert_eq!(conn.url().to_string(), "http://trillium.rs/");
    //
    // let url = url::Url::parse("http://trillium.rs").unwrap();
    // let conn = Conn::new(Method::Post, url, ClientConfig::default()); //<-
    // assert_eq!(conn.method(), Method::Post);
    // assert_eq!(conn.url().to_string(), "http://trillium.rs/");
    //
    // ```
    // */
    // pub fn new<M, U, C>(method: M, url: U, config: C) -> Self
    // where
    //     M: TryInto<Method>,
    //     <M as TryInto<Method>>::Error: Debug,
    //     U: TryInto<Url>,
    //     <U as TryInto<Url>>::Error: Debug,
    //     C: Connector,
    // {
    //     Self::new_with_config(
    //         config.arced(),
    //         method.try_into().unwrap(),
    //         url.try_into().unwrap(),
    //     )
    // }

    pub(crate) fn new_with_config(
        config: Arc<dyn ObjectSafeConnector>,
        method: Method,
        url: Url,
    ) -> Self {
        Self {
            url,
            method,
            request_headers: Headers::new(),
            response_headers: Headers::new(),
            transport: None,
            status: None,
            request_body: None,
            pool: None,
            buffer: Vec::with_capacity(128).into(),
            response_body_state: ReceivedBodyState::Start,
            config,
            headers_finalized: false,
        }
    }

    /**
    retrieves a mutable borrow of the request headers, suitable for
    appending a header. generally, prefer using chainable methods on
    Conn

    ```
    use trillium_testing::ClientConfig;
    use trillium_client::Client;

    let handler = |conn: trillium::Conn| async move {
        let header = conn.headers().get_str("some-request-header").unwrap_or_default();
        let response = format!("some-request-header was {}", header);
        conn.ok(response)
    };

    let client = Client::new(ClientConfig::new());

    trillium_testing::with_server(handler, move |url| async move {
        let mut conn = client.get(url);

        conn.request_headers() //<-
            .insert("some-request-header", "header-value");

        (&mut conn).await?;

        assert_eq!(
            conn.response_body().read_string().await?,
            "some-request-header was header-value"
        );
        Ok(())
    })
    ```
    */
    pub fn request_headers(&mut self) -> &mut Headers {
        &mut self.request_headers
    }

    /**
    chainable setter for [`inserting`](Headers::insert) a request header

    ```
    use trillium_testing::ClientConfig;


    let handler = |conn: trillium::Conn| async move {
        let header = conn.headers().get_str("some-request-header").unwrap_or_default();
        let response = format!("some-request-header was {}", header);
        conn.ok(response)
    };

    let client = trillium_client::Client::new(ClientConfig::new());

    trillium_testing::with_server(handler, |url| async move {
        let mut conn = client.get(url)
            .with_header("some-request-header", "header-value") // <--
            .await?;
        assert_eq!(
            conn.response_body().read_string().await?,
            "some-request-header was header-value"
        );
        Ok(())
    })
    ```
    */

    pub fn with_header(
        mut self,
        name: impl Into<HeaderName<'static>>,
        value: impl Into<HeaderValues>,
    ) -> Self {
        self.request_headers.insert(name, value);
        self
    }

    /**
    chainable setter for `extending` request headers

    ```
    let handler = |conn: trillium::Conn| async move {
        let header = conn.headers().get_str("some-request-header").unwrap_or_default();
        let response = format!("some-request-header was {}", header);
        conn.ok(response)
    };

    use trillium_testing::ClientConfig;
    let client = trillium_client::client(ClientConfig::new());

    trillium_testing::with_server(handler, move |url| async move {
        let mut conn = client.get(url)
            .with_headers([ // <--
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

    pub fn with_headers<HN, HV, I>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = (HN, HV)> + Send,
        HN: Into<HeaderName<'static>>,
        HV: Into<HeaderValues>,
    {
        self.request_headers.extend(headers);
        self
    }

    /**
    ```
    let handler = |conn: trillium::Conn| async move {
        conn.with_header("some-header", "some-value")
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
        Ok(self.with_body(serde_json::to_string(body)?).with_header(
            trillium_http::KnownHeaderName::ContentType,
            "application/json",
        ))
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

    #[allow(clippy::needless_borrow)]
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

    // --- everything below here is private ---

    pub(crate) fn set_pool(&mut self, pool: Pool<Origin, BoxedTransport>) {
        self.pool = Some(pool);
    }

    fn finalize_headers(&mut self) {
        if self.headers_finalized {
            return;
        }

        if self.request_headers.get(Host).is_none() {
            let url = &self.url;
            let host = url.host_str().unwrap().to_owned();

            if let Some(port) = url.port() {
                self.request_headers.insert(Host, format!("{host}:{port}"));
            } else {
                self.request_headers.insert(Host, host);
            };
        }

        self.request_headers.try_insert(UserAgent, USER_AGENT);

        if self.method == Method::Connect {
            self.request_headers.insert(ProxyConnection, "keep-alive");
        }

        match self.pool {
            Some(_) => {
                self.request_headers.insert(Connection, "keep-alive");
            }

            None => {
                self.request_headers.try_insert(Connection, "close");
            }
        }

        if self.method != Method::Get {
            if let Some(len) = self.body_len() {
                if len != 0 {
                    self.request_headers.insert(Expect, "100-continue");
                }
                self.request_headers.insert(ContentLength, len.to_string());
            } else {
                self.request_headers.insert(TransferEncoding, "chunked");
            }
        }
        self.headers_finalized = true;
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
                let mut transport = Connector::connect(&self.config, &self.url).await?;
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

        for (header, values) in self.request_headers.iter() {
            for value in values.iter() {
                write!(buf, "{header}: {value}\r\n")?;
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
                    return Err(Error::PartialHead);
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
        let parse_result = httparse_res.parse(&self.buffer[..head_offset])?;

        match parse_result {
            httparse::Status::Complete(n) if n == head_offset => {}
            _ => return Err(Error::PartialHead),
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
            .contains_ignore_ascii_case("expect", "100-continue")
        {
            log::trace!("Expecting 100-continue");
            self.parse_head().await?;
            if self.status == Some(Status::Continue) {
                self.status = None;
                log::trace!("Received 100-continue, sending request body");
            } else {
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
            Err(Error::UnexpectedHeader("content-length"))
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
                Ok(drain) => log::debug!("drained {}", bytes(drain)),
                Err(e) => log::warn!("failed to drain body, {:?}", e),
            }
        }
    }

    async fn exec(&mut self) -> Result<()> {
        self.finalize_headers();
        self.connect_and_send_head().await?;
        self.send_body_and_parse_head().await?;
        Ok(())
    }
}

fn bytes(bytes: u64) -> String {
    use size::{Base, Size};
    Size::from_bytes(bytes)
        .format()
        .with_base(Base::Base10)
        .to_string()
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
        conn.finalize_headers();
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
        Upgrade {
            request_headers: std::mem::take(&mut conn.request_headers),
            path: conn.url.path().to_string(),
            method: conn.method,
            state: StateSet::new(),
            transport: conn.transport.take().unwrap(),
            buffer: Some(std::mem::take(&mut conn.buffer).into()),
            stopper: Stopper::new(),
        }
    }
}

impl IntoFuture for Conn {
    type Output = Result<Conn>;

    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'static>>;

    fn into_future(mut self) -> Self::IntoFuture {
        Box::pin(async move {
            self.exec().await?;
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
