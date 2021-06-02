use crate::util::encoding;
pub use async_net::TcpStream;
use encoding_rs::Encoding;
use futures_lite::future::poll_once;
use futures_lite::{AsyncReadExt, AsyncWriteExt};
use memmem::{Searcher, TwoWaySearcher};
use std::borrow::Cow;
use std::convert::TryInto;
use std::fmt::{self, Debug, Formatter};
use std::io::Write;
use trillium::http_types::content::ContentLength;
use trillium::http_types::headers::{Headers, CONTENT_LENGTH, HOST, TRANSFER_ENCODING};
use trillium::http_types::{Body, Extensions, Method, StatusCode};
use trillium_http::{BodyEncoder, ReceivedBody, ReceivedBodyState, Upgrade};
use trillium_http::{Error, Result, Stopper};

use url::Url;

use crate::{pool::PoolEntry, Connector, Pool};

const MAX_HEADERS: usize = 128;
const MAX_HEAD_LENGTH: usize = 8 * 1024;

pub struct Conn<'config, C: Connector> {
    url: Url,
    method: Method,
    request_headers: Headers,
    response_headers: Headers,
    transport: Option<C::Transport>,
    status: Option<StatusCode>,
    request_body: Option<Body>,
    pool: Option<Pool<C::Transport>>,
    buffer: Option<Vec<u8>>,
    response_body_state: ReceivedBodyState,
    config: Option<Cow<'config, C::Config>>,
}

macro_rules! method {
    ($fn_name:ident, $method:ident) => {
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
    pub fn set_config<'c2: 'config>(&mut self, config: &'c2 C::Config) {
        self.config = Some(Cow::Borrowed(config));
    }

    pub fn with_config<'c2: 'config>(mut self, config: &'c2 C::Config) -> Conn<'config, C> {
        self.set_config(config);
        self
    }
}

impl<C: Connector> Conn<'static, C> {
    pub async fn execute(mut self) -> Result<Self> {
        self.finalize_headers();
        self.connect_and_send_head().await?;
        self.send_body_and_parse_head().await?;
        Ok(self)
    }
}

impl<C: Connector> Conn<'_, C> {
    pub fn new<U>(method: Method, url: U) -> Self
    where
        <U as TryInto<Url>>::Error: Debug,
        U: TryInto<Url>,
    {
        Self {
            url: url.try_into().unwrap(),
            method,
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

    pub fn request_headers(&mut self) -> &mut Headers {
        &mut self.request_headers
    }
    pub fn response_headers(&self) -> &Headers {
        &self.response_headers
    }

    pub fn set_pool(&mut self, pool: Pool<C::Transport>) {
        self.pool = Some(pool);
    }

    pub fn with_pool(mut self, pool: Pool<C::Transport>) -> Self {
        self.set_pool(pool);
        self
    }

    fn finalize_headers(&mut self) {
        if self.request_headers.get(HOST).is_none() {
            let url = &self.url;
            let host = url.host_str().unwrap().to_owned();

            if let Some(port) = url.port() {
                self.request_headers
                    .insert(HOST, format!("{}:{}", host, port));
            } else {
                self.request_headers.insert(HOST, host);
            };
        }

        if self.request_headers.get("user-agent").is_none() {
            self.request_headers.insert("user-agent", USER_AGENT);
        }

        if self.method == Method::Connect {
            self.request_headers
                .insert("proxy-connection", "keep-alive");
        }

        match self.pool {
            Some(_) => {
                self.request_headers.insert("connection", "keep-alive");
            }
            None => {
                if self.request_headers.get("connection").is_none() {
                    self.request_headers.insert("connection", "close");
                }
            }
        }

        if Some(0) != self.body_len() {
            self.request_headers.insert("expect", "100-continue");
        }

        if self.method != Method::Get {
            if let Some(len) = self.body_len() {
                self.request_headers.insert(CONTENT_LENGTH, len.to_string());
            } else {
                self.request_headers.insert(TRANSFER_ENCODING, "chunked");
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

    pub fn set_request_body(&mut self, body: impl Into<Body>) {
        self.request_body = Some(body.into());
    }

    pub fn with_request_body(mut self, body: impl Into<Body>) -> Self {
        self.set_request_body(body);
        self
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
                    && candidate.write_all(&head).await.is_ok()
                {
                    return Some(candidate);
                }
            }
        }
        None
    }

    async fn connect_and_send_head(&mut self) -> Result<()> {
        if self.transport.is_some() {
            panic!("cannot connect a second time");
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

    pub async fn build_head(&mut self) -> Result<Vec<u8>> {
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

        let mut headers = self.request_headers.iter().collect::<Vec<_>>();
        headers.sort_unstable_by_key(|(h, _)| if **h == HOST { "0" } else { h.as_str() });
        for (header, values) in headers {
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
        for header in httparse_res.headers {
            self.response_headers
                .insert(header.name, std::str::from_utf8(header.value)?);
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
            if self.status == Some(StatusCode::Continue) {
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
        if let Some(body) = self.request_body.take() {
            futures_lite::io::copy(BodyEncoder::new(body), self.transport()).await?;
        }
        Ok(())
    }

    pub fn request_encoding(&self) -> &'static Encoding {
        encoding(&self.request_headers)
    }

    pub fn response_encoding(&self) -> &'static Encoding {
        encoding(&self.response_headers)
    }

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

    pub fn response_content_length(&self) -> Option<u64> {
        ContentLength::from_headers(&self.response_headers)
            .ok()
            .flatten()
            .map(|cl| cl.len())
    }

    fn validate_response_headers(&self) -> Result<()> {
        let content_length = ContentLength::from_headers(&self.response_headers)
            .map_err(|_| Error::MalformedHeader("content-length"))?;

        let transfer_encoding_chunked = self
            .response_headers
            .contains_ignore_ascii_case(TRANSFER_ENCODING, "chunked");

        if content_length.is_some() && transfer_encoding_chunked {
            Err(Error::UnexpectedHeader("content-length"))
        } else {
            Ok(())
        }
    }

    fn is_keep_alive(&self) -> bool {
        self.response_headers
            .get("connection")
            .map(|value| value.as_str().to_lowercase().contains("keep-alive"))
            .unwrap_or_default()
    }

    pub async fn send(&mut self) -> Result<()> {
        self.finalize_headers();
        self.connect_and_send_head().await?;
        self.send_body_and_parse_head().await?;

        Ok(())
    }

    pub fn status(&self) -> Option<StatusCode> {
        self.status
    }

    pub fn into_inner(mut self) -> C::Transport {
        self.transport.take().unwrap()
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

    pub async fn recycle(mut self) {
        if self.is_keep_alive() && self.transport.is_some() && self.pool.is_some() {
            self.finish_reading_body().await;
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
        if self.response_body_state == ReceivedBodyState::End
            && self.is_keep_alive()
            && self.transport.is_some()
            && self.pool.is_some()
        {
            let pool = self.pool.take().unwrap();
            let transport = self.transport.take().unwrap();
            pool.insert(
                C::peer_addr(&transport).unwrap(),
                PoolEntry::new(transport, None),
            );
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
            state: Extensions::new(),
            transport: conn.transport.take().unwrap(),
            buffer: conn.buffer.take(),
            stopper: Stopper::new(),
        }
    }
}
