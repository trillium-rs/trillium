use crate::{pool::PoolEntry, util::encoding, Connector, Pool};
use encoding_rs::Encoding;
use futures_lite::{future::poll_once, io, AsyncReadExt, AsyncWriteExt};
use memmem::{Searcher, TwoWaySearcher};
use std::{
    borrow::Cow,
    convert::TryInto,
    fmt::{self, Debug, Formatter},
    io::{ErrorKind, Write},
    str::FromStr,
};
use trillium_http::{
    Body, Error, HeaderName, HeaderValue, Headers,
    KnownHeaderName::{
        Connection, ContentLength, Expect, Host, ProxyConnection, TransferEncoding, UserAgent,
    },
    Method, ReceivedBody, ReceivedBodyState, Result, StateSet, Status, Stopper, Upgrade,
};
use url::Url;

const MAX_HEADERS: usize = 128;
const MAX_HEAD_LENGTH: usize = 2 * 1024;

/**
a client connection, representing both an outbound http request and a
http response
*/

pub struct Conn<'config, C: Connector> {
    url: Url,
    method: Method,
    request_headers: Headers,
    response_headers: Headers,
    transport: Option<C::Transport>,
    status: Option<Status>,
    request_body: Option<Body>,
    pool: Option<Pool<C::Transport>>,
    buffer: Option<Vec<u8>>,
    response_body_state: ReceivedBodyState,
    config: Option<Cow<'config, C::Config>>,
}

macro_rules! method {
    ($fn_name:ident, $method:ident) => {
        method!(
            $fn_name,
            $method,
            concat!(
                // yep, macro-generated doctests
                "Builds a new client conn with the ",
                stringify!($fn_name),
                " http method and the provided url.

```
use trillium_testing::prelude::*;
type Conn = trillium_client::Conn<'static, trillium_smol::TcpConnector>;

let conn = Conn::",
                stringify!($fn_name),
                "(\"http://localhost:8080/some/route\");

assert_eq!(conn.method(), Method::",
                stringify!($method),
                ");
assert_eq!(conn.url().to_string(), \"http://localhost:8080/some/route\");
```
"
            )
        );
    };
    ($fn_name:ident, $method:ident, $doc_comment:expr) => {
        #[doc = $doc_comment]
        pub fn $fn_name<U>(url: U) -> Self
        where
            <U as TryInto<Url>>::Error: Debug,
            U: TryInto<Url>,
        {
            Self::new(Method::$method, url)
        }
    };
}

const USER_AGENT: &str = concat!("trillium-client/", env!("CARGO_PKG_VERSION"));

impl<C: Connector> Debug for Conn<'_, C> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conn")
            .field("url", &self.url)
            .field("method", &self.method)
            .field("request_headers", &self.request_headers)
            .field("response_headers", &self.response_headers)
            .field("status", &self.status)
            .field("request_body", &self.request_body)
            .field("pool", &self.pool)
            .field(
                "buffer",
                &self.buffer.as_deref().map(String::from_utf8_lossy),
            )
            .field("response_body_state", &self.response_body_state)
            .field("config", &self.config)
            .finish()
    }
}

impl<'config, C: Connector> Conn<'config, C> {
    /**
    imperatively assign a given config reference to this Conn.

    ```
    use trillium_smol::{TcpConnector, ClientConfig};
    type Conn<'config> = trillium_client::Conn<'config, TcpConnector>;

    let config = ClientConfig {
        ttl: Some(100),
        ..Default::default()
    };

    let mut conn = Conn::get("http://localhost:8080/");
    conn.set_config(&config); // <-
    ```
     */
    pub fn set_config<'c2: 'config>(&mut self, config: &'c2 C::Config) {
        self.config = Some(Cow::Borrowed(config));
    }

    /**
    set a config reference on this conn and return the conn, allowing chaining
    ```
    use trillium_smol::{TcpConnector, ClientConfig};
    type Conn<'config> = trillium_client::Conn<'config, TcpConnector>;

    let config = ClientConfig {
        nodelay: Some(true),
        ..Default::default()
    };

    let conn = Conn::get("http://localhost:8080/")
        .with_config(&config); //<-
    ```
     */
    pub fn with_config<'c2: 'config>(mut self, config: &'c2 C::Config) -> Conn<'config, C> {
        self.set_config(config);
        self
    }
}

impl<C: Connector> Conn<'static, C> {
    /**
    Performs the http request, consuming and returning the conn. This
    is suitable for chaining on conns with owned Config. For a
    borrowed equivalent of this, see [`Conn::send`].
    ```
    type Conn = trillium_client::Conn<'static, trillium_smol::TcpConnector>;

    trillium_testing::with_server("ok", |url| async move {
        let mut conn = Conn::get(url).execute().await?; //<-
        assert_eq!(conn.status().unwrap(), 200);
        assert_eq!(conn.response_body().read_string().await?, "ok");
        Ok(())
    });
    ```
     */
    pub async fn execute(mut self) -> Result<Self> {
        self.finalize_headers();
        self.connect_and_send_head().await?;
        self.send_body_and_parse_head().await?;
        Ok(self)
    }
}

impl<C: Connector> Conn<'_, C> {
    /**
    builds a new client Conn with the provided method and url
    ```
    type Conn = trillium_client::Conn<'static, trillium_smol::TcpConnector>;
    use trillium_testing::prelude::*;

    let conn = Conn::new("get", "http://trillium.rs"); //<-
    assert_eq!(conn.method(), Method::Get);
    assert_eq!(conn.url().to_string(), "http://trillium.rs/");

    let url = url::Url::parse("http://trillium.rs").unwrap();
    let conn = Conn::new(Method::Post, url); //<-
    assert_eq!(conn.method(), Method::Post);
    assert_eq!(conn.url().to_string(), "http://trillium.rs/");

    ```
    */
    pub fn new<M, U>(method: M, url: U) -> Self
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: Debug,
        U: TryInto<Url>,
        <U as TryInto<Url>>::Error: Debug,
    {
        Self {
            url: url.try_into().expect("could not parse url"),
            method: method.try_into().expect("did not recognize method"),
            request_headers: Headers::new(),
            response_headers: Headers::new(),
            transport: None,
            status: None,
            request_body: None,
            pool: None,
            buffer: None,
            response_body_state: ReceivedBodyState::Start,
            config: None,
        }
    }

    method!(get, Get);
    method!(post, Post);
    method!(put, Put);
    method!(delete, Delete);
    method!(patch, Patch);

    /**
    Performs the http request on a mutable borrow of the conn. This is
    suitable for conns with borrowed Config. For an owned and
    chainable equivalent of this, see [`Conn::execute`].

    ```
    use trillium_smol::TcpConnector;
    type Client = trillium_client::Client<TcpConnector>;
    trillium_testing::with_server("ok", |url| async move {
        let client = Client::new();
        let mut conn = client.get(url);

        conn.send().await?; //<-

        assert_eq!(conn.status().unwrap(), 200);
        assert_eq!(conn.response_body().read_string().await?, "ok");
        Ok(())
    })
    ```
     */

    pub async fn send(&mut self) -> Result<()> {
        self.finalize_headers();
        self.connect_and_send_head().await?;
        self.send_body_and_parse_head().await?;

        Ok(())
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

    /**
    retrieves a mutable borrow of the request headers, suitable for
    appending a header

    ```
    use trillium_smol::TcpConnector;
    type Conn = trillium_client::Conn<'static, TcpConnector>;

    let handler = |conn: trillium::Conn| async move {
        let header = conn.headers().get_str("some-request-header").unwrap_or_default();
        let response = format!("some-request-header was {}", header);
        conn.ok(response)
    };

    trillium_testing::with_server(handler, |url| async move {
        let mut conn = Conn::get(url);

        conn.request_headers() //<-
            .insert("some-request-header", "header-value");

        conn.send().await?;
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
    ```
    use trillium_smol::TcpConnector;
    type Conn = trillium_client::Conn<'static, TcpConnector>;

    let handler = |conn: trillium::Conn| async move {
        conn.with_header("some-header", "some-value")
            .with_status(200)
    };

    trillium_testing::with_server(handler, |url| async move {
        let conn = Conn::get(url).execute().await?;

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
    ```
    env_logger::init();
    use trillium_smol::TcpConnector;
    type Conn = trillium_client::Conn<'static, TcpConnector>;

    let handler = |mut conn: trillium::Conn| async move {
        let body = conn.request_body_string().await.unwrap();
        conn.ok(format!("request body was: {}", body))
    };

    trillium_testing::with_server(handler, |url| async move {
        let mut conn = Conn::post(url);

        conn.set_request_body("body"); //<-

        conn.send().await?;

        assert_eq!(conn.response_body().read_string().await?, "request body was: body");
        Ok(())
    });
    ```
     */
    pub fn set_request_body(&mut self, body: impl Into<Body>) {
        self.request_body = Some(body.into());
    }

    /**
    ```
    env_logger::init();
    use trillium_smol::TcpConnector;
    type Conn = trillium_client::Conn<'static, TcpConnector>;

    let handler = |mut conn: trillium::Conn| async move {
        let body = conn.request_body_string().await.unwrap();
        conn.ok(format!("request body was: {}", body))
    };

    trillium_testing::with_server(handler, |url| async move {
        let mut conn = Conn::post(url)
            .with_request_body("body") //<-
            .execute()
            .await?;

        assert_eq!(
            conn.response_body().read_string().await?,
            "request body was: body"
        );
        Ok(())
    });
    ```
    */
    pub fn with_request_body(mut self, body: impl Into<Body>) -> Self {
        self.set_request_body(body);
        self
    }

    // pub(crate) fn request_encoding(&self) -> &'static Encoding {
    //     encoding(&self.request_headers)
    // }

    pub(crate) fn response_encoding(&self) -> &'static Encoding {
        encoding(&self.response_headers)
    }

    /**
    retrieves the url for this conn.
    ```
    use trillium_smol::TcpConnector;
    use trillium_client::Conn;

    let conn = Conn::<TcpConnector>::get("http://localhost:9080");

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
    use trillium_smol::TcpConnector;
    use trillium_client::Conn;
    use trillium_testing::prelude::*;
    let conn = Conn::<TcpConnector>::get("http://localhost:9080");

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
    use trillium_smol::TcpConnector;
    type Conn = trillium_client::Conn<'static, TcpConnector>;

    let handler = |mut conn: trillium::Conn| async move {
        conn.ok("hello from trillium")
    };

    trillium_testing::with_server(handler, |url| async move {
        let mut conn = Conn::get(url).execute().await?;

        let response_body = conn.response_body(); //<-

        assert_eq!(19, response_body.content_length().unwrap());
        let string = response_body.read_string().await?;
        assert_eq!("hello from trillium", string);
        Ok(())
    });
    ```
     */

    pub fn response_body(&mut self) -> ReceivedBody<'_, C::Transport> {
        ReceivedBody::new(
            self.response_content_length(),
            &mut self.buffer,
            self.transport.as_mut().unwrap(),
            &mut self.response_body_state,
            None,
            encoding(&self.response_headers),
        )
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
    use trillium_smol::TcpConnector;
    use trillium_testing::prelude::*;
    type Conn = trillium_client::Conn<'static, TcpConnector>;
    async fn handler(conn: trillium::Conn) -> trillium::Conn {
        conn.with_status(418)
    }

    trillium_testing::with_server(handler, |url| async move {
        let conn = Conn::get(url).execute().await?;
        assert_eq!(Status::ImATeapot, conn.status().unwrap());
        Ok(())
    });
    ```
     */
    pub fn status(&self) -> Option<Status> {
        self.status
    }

    // --- everything below here is private ---

    pub(crate) fn set_pool(&mut self, pool: Pool<C::Transport>) {
        self.pool = Some(pool);
    }

    fn finalize_headers(&mut self) {
        if self.request_headers.get(Host).is_none() {
            let url = &self.url;
            let host = url.host_str().unwrap().to_owned();

            if let Some(port) = url.port() {
                self.request_headers
                    .insert(Host, format!("{}:{}", host, port));
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
    }

    fn body_len(&self) -> Option<u64> {
        if let Some(ref body) = self.request_body {
            body.len()
        } else {
            Some(0)
        }
    }

    async fn find_pool_candidate(
        &self,
        socket_addrs: &[std::net::SocketAddr],
        head: &[u8],
    ) -> Option<C::Transport> {
        let mut byte = [0];
        if let Some(pool) = &self.pool {
            for mut candidate in pool.candidates(&socket_addrs) {
                if poll_once(candidate.read(&mut byte)).await.is_none()
                    && candidate.write_all(head).await.is_ok()
                {
                    return Some(candidate);
                }
            }
        }
        None
    }

    async fn connect_and_send_head(&mut self) -> Result<()> {
        if self.transport.is_some() {
            return Err(Error::Io(std::io::Error::new(
                ErrorKind::AlreadyExists,
                "conn already connected",
            )));
        }

        let head = self.build_head().await?;
        let socket_addrs = self.url.socket_addrs(|| None)?;

        let transport = match self.find_pool_candidate(&socket_addrs[..], &head).await {
            Some(transport) => {
                log::debug!("reusing connection to {}", C::peer_addr(&transport)?);
                transport
            }

            None => {
                let config = if let Some(config) = &self.config {
                    config.clone()
                } else {
                    Cow::Owned(C::Config::default())
                };

                let mut transport = C::connect(&self.url, &*config).await?;
                log::debug!("opened new connection to {}", C::peer_addr(&transport)?);
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
        write!(buf, "{} ", method)?;

        if method == Method::Connect {
            let host = url.host_str().ok_or(Error::UnexpectedUriFormat)?;

            let port = url
                .port_or_known_default()
                .ok_or(Error::UnexpectedUriFormat)?;

            write!(buf, "{}:{}", host, port)?;
        } else {
            write!(buf, "{}", url.path())?;
            if let Some(query) = url.query() {
                write!(buf, "?{}", query)?;
            }
        }

        write!(buf, " HTTP/1.1\r\n")?;

        for (header, values) in self.request_headers.iter() {
            for value in values.iter() {
                write!(buf, "{}: {}\r\n", header, value)?;
            }
        }

        write!(buf, "\r\n")?;
        log::trace!(
            "{}",
            std::str::from_utf8(&buf).unwrap().replace("\r\n", "\r\n> ")
        );

        Ok(buf)
    }

    fn transport(&mut self) -> &mut C::Transport {
        self.transport.as_mut().unwrap()
    }

    async fn read_head(&mut self) -> Result<(Vec<u8>, Vec<u8>)> {
        let mut buf = self.buffer.take().unwrap_or_default();
        let mut len = 0;
        let searcher = TwoWaySearcher::new(b"\r\n\r\n");
        loop {
            buf.extend(std::iter::repeat(0).take(100));
            let bytes = self.transport().read(&mut buf[len..]).await?;

            let search_start = len.max(3) - 3;
            let search = searcher.search_in(&buf[search_start..]);

            if let Some(index) = search {
                buf.truncate(len + bytes);

                log::trace!(
                    "{}",
                    String::from_utf8_lossy(&buf[..search_start + index]).replace("\r\n", "\r\n< ")
                );

                let body = buf.split_off(search_start + index + 4);

                return Ok((buf, body));
            }

            len += bytes;

            if bytes == 0 {
                if len == 0 {
                    return Err(Error::Closed);
                } else {
                    log::debug!(
                        "disconnect? partial head content: \n\n{:?}",
                        String::from_utf8_lossy(&buf[..])
                    );
                    return Err(Error::PartialHead);
                }
            }

            if len >= MAX_HEAD_LENGTH {
                return Err(Error::HeadersTooLong);
            }
        }
    }

    async fn parse_head(&mut self) -> Result<()> {
        let (head, body) = self.read_head().await?;
        self.buffer = if body.is_empty() { None } else { Some(body) };
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut httparse_res = httparse::Response::new(&mut headers);
        let status = httparse_res.parse(&head[..]);

        if let Err(e) = status {
            log::error!("{:?}", e);
            log::error!(
                "partial head content: {}",
                String::from_utf8_lossy(&head[..])
            );
        }

        let status = status?;

        if status.is_partial() {
            log::trace!(
                "partial head content: {}",
                String::from_utf8_lossy(&head[..])
            );
            return Err(Error::PartialHead);
        }

        self.status = httparse_res.code.map(|code| code.try_into().unwrap());

        self.response_headers.reserve(httparse_res.headers.len());
        for header in httparse_res.headers {
            let header_name = HeaderName::from_str(header.name)?;
            let header_value = HeaderValue::from(header.value.to_owned());
            self.response_headers.append(header_name, header_value);
        }

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
                log::trace!("Received 100-continue, sending request body");
            } else {
                log::trace!(
                    "Received a status code other than 100-continue, not sending request body"
                );
                return Ok(());
            }
        }

        self.send_body().await?;
        self.parse_head().await?;

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
}

impl<C: Connector> AsRef<C::Transport> for Conn<'_, C> {
    fn as_ref(&self) -> &C::Transport {
        self.transport.as_ref().unwrap()
    }
}

fn bytes(bytes: u64) -> String {
    use size::{Base, Size, Style};
    Size::to_string(&Size::Bytes(bytes), Base::Base10, Style::Smart)
}

impl<C: Connector> Drop for Conn<'_, C> {
    fn drop(&mut self) {
        if !self.is_keep_alive() || self.transport.is_none() || self.pool.is_none() {
            return;
        }

        let transport = self.transport.take().unwrap();
        let peer_addr = C::peer_addr(&transport).unwrap();
        let pool = self.pool.take().unwrap();

        if self.response_body_state == ReceivedBodyState::End {
            log::trace!("response body has been read to completion, checking transport back into pool for {}", &peer_addr);
            pool.insert(peer_addr, PoolEntry::new(transport, None));
        } else {
            let content_length = self.response_content_length();
            let buffer = self.buffer.take();
            let response_body_state = self.response_body_state;
            let encoding = encoding(&self.response_headers);
            C::spawn(async move {
                let mut response_body = ReceivedBody::<C::Transport>::new(
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
                        pool.insert(peer_addr, PoolEntry::new(transport, None));
                    }

                    Err(ioerror) => log::error!("unable to recycle conn due to {}", ioerror),
                }
            });
        }
    }
}

impl<C: Connector> From<Conn<'_, C>> for Body {
    fn from(conn: Conn<'_, C>) -> Body {
        let received_body: ReceivedBody<'static, _> = conn.into();
        received_body.into()
    }
}

impl<C: Connector> From<Conn<'_, C>> for ReceivedBody<'static, C::Transport> {
    fn from(mut conn: Conn<'_, C>) -> Self {
        conn.finalize_headers();
        ReceivedBody::new(
            conn.response_content_length(),
            conn.buffer.take(),
            conn.transport.take().unwrap(),
            conn.response_body_state,
            conn.pool
                .take()
                .map(|pool| -> Box<dyn Fn(C::Transport) + Send + Sync> {
                    Box::new(move |transport| {
                        pool.insert(
                            C::peer_addr(&transport).unwrap(),
                            PoolEntry::new(transport, None),
                        );
                    })
                }),
            conn.response_encoding(),
        )
    }
}

impl<C: Connector> From<Conn<'_, C>> for Upgrade<C::Transport> {
    fn from(mut conn: Conn<'_, C>) -> Self {
        Upgrade {
            request_headers: std::mem::replace(&mut conn.request_headers, Headers::new()),
            path: conn.url.path().to_string(),
            method: conn.method,
            state: StateSet::new(),
            transport: conn.transport.take().unwrap(),
            buffer: conn.buffer.take(),
            stopper: Stopper::new(),
        }
    }
}
