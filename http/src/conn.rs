use encoding_rs::Encoding;
use futures_lite::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use http_types::headers::{CONTENT_LENGTH, CONTENT_TYPE, HOST, UPGRADE};
use http_types::transfer::Encoding::Chunked;
use http_types::{
    content::ContentLength,
    headers::{Header, Headers, DATE, EXPECT, TRANSFER_ENCODING},
    other::Date,
    transfer::TransferEncoding,
    Body, Extensions, Method, StatusCode, Url, Version,
};
use httparse::{Request, EMPTY_HEADER};
use memmem::{Searcher, TwoWaySearcher};
use std::future::Future;
use std::iter;
use std::{
    convert::TryInto,
    fmt::{self, Debug, Formatter},
};

use crate::util::encoding;
use crate::{
    body_encoder::BodyEncoder, Error, ReceivedBody, ReceivedBodyState, Result, Stopper, Upgrade,
};

const MAX_HEADERS: usize = 128;
const MAX_HEAD_LENGTH: usize = 8 * 1024;
const SERVER: &str = concat!("trillium/", env!("CARGO_PKG_VERSION"));

#[derive(Debug)]
pub enum ConnectionStatus<RW> {
    Close,
    Conn(Conn<RW>),
    Upgrade(Upgrade<RW>),
}

pub struct Conn<RW> {
    pub(crate) request_headers: Headers,
    pub(crate) response_headers: Headers,
    pub(crate) path: String,
    pub(crate) method: Method,
    pub(crate) status: Option<StatusCode>,
    pub(crate) version: Version,
    pub(crate) state: Extensions,
    pub(crate) response_body: Option<Body>,
    pub(crate) rw: RW,
    pub(crate) buffer: Option<Vec<u8>>,
    pub(crate) request_body_state: ReceivedBodyState,
    pub(crate) secure: bool,
    pub(crate) stopper: Stopper,
}

impl<RW> Debug for Conn<RW> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conn")
            .field("request_headers", &self.request_headers)
            .field("response_headers", &self.response_headers)
            .field("path", &self.path)
            .field("method", &self.method)
            .field("status", &self.status)
            .field("version", &self.version)
            .field("request_body_state", &self.request_body_state)
            .finish()
    }
}

impl<RW> Conn<RW>
where
    RW: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    pub fn state(&self) -> &Extensions {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut Extensions {
        &mut self.state
    }

    pub fn request_headers(&self) -> &Headers {
        &self.request_headers
    }

    pub fn response_headers(&mut self) -> &mut Headers {
        &mut self.response_headers
    }

    pub fn set_status(&mut self, status: impl TryInto<http_types::StatusCode>) {
        self.status = status.try_into().ok();
    }

    pub fn path(&self) -> &str {
        self.path.split('?').next().unwrap()
    }

    pub fn host(&self) -> Option<String> {
        self.request_headers.get(HOST).map(|v| v.to_string())
    }

    pub fn url(&self) -> Result<Url> {
        let path = self.path();
        let host = self.host().unwrap_or_else(|| String::from("_"));
        let method = self.method();
        if path.starts_with("http://") || path.starts_with("https://") {
            Ok(Url::parse(path)?)
        } else if path.starts_with('/') {
            Ok(Url::parse(&format!("http://{}{}", host, path))?)
        } else if method == &Method::Connect {
            Ok(Url::parse(&format!("http://{}/", path))?)
        } else {
            Err(Error::UnexpectedUriFormat)
        }
    }

    pub fn set_body(&mut self, body: impl Into<Body>) {
        let body = body.into();

        if self.response_headers.get(CONTENT_TYPE).is_none() {
            self.response_headers
                .insert(CONTENT_TYPE, body.mime().clone());
        }

        self.response_body = Some(body);
    }

    pub fn take_response_body(&mut self) -> Option<Body> {
        self.response_body.take()
    }

    pub fn method(&self) -> &Method {
        &self.method
    }

    pub fn status(&self) -> Option<&StatusCode> {
        self.status.as_ref()
    }

    pub fn response_body(&self) -> Option<&Body> {
        self.response_body.as_ref()
    }

    fn needs_100_continue(&self) -> bool {
        self.request_body_state == ReceivedBodyState::Start
            && self
                .request_headers
                .contains_ignore_ascii_case(EXPECT, "100-continue")
    }

    fn build_request_body(&mut self) -> ReceivedBody<'_, RW> {
        ReceivedBody::new(
            self.request_content_length().ok().flatten(),
            &mut self.buffer,
            &mut self.rw,
            &mut self.request_body_state,
            None,
            encoding(&self.request_headers),
        )
    }

    pub fn request_encoding(&self) -> &'static Encoding {
        encoding(&self.request_headers)
    }

    pub fn response_encoding(&self) -> &'static Encoding {
        encoding(&self.response_headers)
    }

    pub async fn request_body(&mut self) -> ReceivedBody<'_, RW> {
        if self.needs_100_continue() {
            self.send_100_continue().await.ok();
        }

        self.build_request_body()
    }

    pub fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }

    pub async fn map<F, Fut>(rw: RW, stopper: Stopper, f: F) -> crate::Result<Option<Upgrade<RW>>>
    where
        F: Fn(Conn<RW>) -> Fut,
        Fut: Future<Output = Conn<RW>> + Send,
    {
        let mut conn = Conn::new(rw, None, stopper).await?;

        loop {
            conn = match f(conn).await.encode().await? {
                ConnectionStatus::Upgrade(upgrade) => return Ok(Some(upgrade)),
                ConnectionStatus::Close => return Ok(None),
                ConnectionStatus::Conn(next) => next,
            }
        }
    }

    pub fn validate_headers(request_headers: &Headers) -> Result<()> {
        let content_length = ContentLength::from_headers(request_headers)
            .map_err(|_| Error::MalformedHeader("content-length"))?;

        let transfer_encoding_chunked =
            request_headers.contains_ignore_ascii_case(TRANSFER_ENCODING, "chunked");

        if content_length.is_some() && transfer_encoding_chunked {
            Err(Error::UnexpectedHeader("content-length"))
        } else {
            Ok(())
        }
    }

    pub async fn new(rw: RW, bytes: Option<Vec<u8>>, stopper: Stopper) -> Result<Self> {
        let (rw, buf, extra_bytes) = Self::head(rw, bytes, &stopper).await?;
        let buffer = if extra_bytes.is_empty() {
            None
        } else {
            Some(extra_bytes)
        };
        let mut headers = [EMPTY_HEADER; MAX_HEADERS];
        let mut httparse_req = Request::new(&mut headers);
        let status = httparse_req.parse(&buf[..])?;
        if status.is_partial() {
            log::debug!("partial head content: {}", utf8(&buf[..]));
            return Err(Error::PartialHead);
        }

        let method = httparse_req
            .method
            .ok_or(Error::MissingMethod)?
            .parse()
            .map_err(|_| Error::UnrecognizedMethod(httparse_req.method.unwrap().to_string()))?;

        let version = match httparse_req.version {
            Some(1) => Version::Http1_1,
            Some(version) => return Err(Error::UnsupportedVersion(version)),
            None => return Err(Error::MissingVersion),
        };

        let mut request_headers = Headers::new();
        for header in httparse_req.headers.iter() {
            request_headers.insert(header.name, std::str::from_utf8(header.value)?);
        }

        Self::validate_headers(&request_headers)?;

        log::trace!("parsed headers: {:#?}", &request_headers);
        let path = httparse_req
            .path
            .ok_or(Error::RequestPathMissing)?
            .to_owned();

        Ok(Self {
            rw,
            request_headers,
            method,
            version,
            path,
            buffer,
            response_headers: Headers::new(),
            status: None,
            state: Extensions::new(),
            response_body: None,
            request_body_state: ReceivedBodyState::Start,
            secure: false,
            stopper,
        })
    }

    pub fn is_secure(&self) -> bool {
        self.secure
    }

    async fn send_100_continue(&mut self) -> Result<()> {
        log::trace!("sending 100-continue");
        Ok(self.rw.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").await?)
    }

    async fn head(
        mut rw: RW,
        bytes: Option<Vec<u8>>,
        stopper: &Stopper,
    ) -> Result<(RW, Vec<u8>, Vec<u8>)> {
        let mut buf = bytes.unwrap_or_default();
        let mut len = 0;

        let searcher = TwoWaySearcher::new(b"\r\n\r\n");
        loop {
            buf.extend(iter::repeat(0).take(100));
            let bytes = if len == 0 {
                stopper
                    .stop_future(rw.read(&mut buf[len..]))
                    .await
                    .ok_or(Error::Closed)??
            } else {
                rw.read(&mut buf[len..]).await?
            };

            let search_start = len.max(3) - 3;
            let search = searcher.search_in(&buf[search_start..]);

            if let Some(index) = search {
                buf.truncate(len + bytes);
                log::trace!(
                    "in head, finished headers:\n {}",
                    utf8(&buf[..search_start + index])
                );
                let body = buf.split_off(search_start + index + 4);
                if !body.is_empty() {
                    log::trace!("read the front of the body: {}", utf8(&body));
                }
                return Ok((rw, buf, body));
            }

            len += bytes;

            if bytes == 0 {
                if len == 0 {
                    return Err(Error::Closed);
                } else {
                    log::debug!(
                        "disconnect? partial head content: \n{:?}",
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

    pub fn inner_mut(&mut self) -> &mut RW {
        &mut self.rw
    }

    pub async fn next(mut self) -> Result<Self> {
        if !self.needs_100_continue() || self.request_body_state != ReceivedBodyState::Start {
            self.build_request_body().drain().await?;
        }
        Conn::new(self.rw, self.buffer, self.stopper).await
    }

    fn should_close(&self) -> bool {
        self.request_headers
            .contains_ignore_ascii_case("connection", "close")
            || self
                .response_headers
                .contains_ignore_ascii_case("connection", "close")
    }

    fn should_upgrade(&self) -> bool {
        let has_upgrade_header = self.request_headers.get(UPGRADE).is_some();
        let connection_upgrade = match self.request_headers.get("connection") {
            Some(h) => h
                .as_str()
                .split(',')
                .any(|h| h.eq_ignore_ascii_case("upgrade")),
            None => false,
        };
        let response_is_switching_protocols = self.status == Some(StatusCode::SwitchingProtocols);

        has_upgrade_header && connection_upgrade && response_is_switching_protocols
    }

    pub async fn finish(self) -> Result<ConnectionStatus<RW>> {
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

    pub fn request_content_length(&self) -> crate::Result<Option<u64>> {
        if self
            .request_headers
            .contains_ignore_ascii_case(TRANSFER_ENCODING, "chunked")
        {
            Ok(None)
        } else if let Some(cl) = ContentLength::from_headers(&self.request_headers)
            .map_err(|_| Error::MalformedHeader("content-length"))?
            .map(|cl| cl.len())
        {
            Ok(Some(cl))
        } else if self.method == Method::Get {
            Ok(Some(0))
        } else {
            Err(Error::HeaderMissing("content-length or transfer-encoding"))
        }
    }

    pub async fn encode(mut self) -> Result<ConnectionStatus<RW>> {
        self.send_headers().await?;

        if self.method() != &Method::Head {
            if let Some(body) = self.response_body.take() {
                io::copy(BodyEncoder::new(body), &mut self.rw).await?;
            }
        }

        self.finish().await
    }

    fn body_len(&self) -> Option<u64> {
        match self.response_body {
            Some(ref body) => body.len(),
            None => Some(0),
        }
    }

    fn finalize_headers(&mut self) {
        if let Some(len) = self.body_len() {
            self.response_headers.apply(ContentLength::new(len));
        }

        if self.response_headers.get(CONTENT_LENGTH).is_none() {
            self.response_headers.apply(TransferEncoding::new(Chunked));
        } else {
            self.response_headers.remove(TRANSFER_ENCODING);
        }

        if self.response_headers.get("server").is_none() {
            self.response_headers.insert("server", SERVER);
        }

        if self.stopper.is_stopped() {
            self.response_headers.insert("connection", "close");
        } else if self.response_headers.get("connection").is_none()
            && !self
                .request_headers
                .contains_ignore_ascii_case("connection", "close")
        {
            self.response_headers.insert("connection", "keep-alive");
        }

        if self.response_headers.get(DATE).is_none() {
            Date::now().apply_header(&mut self.response_headers);
        }
    }

    async fn send_headers(&mut self) -> Result<()> {
        let status = self.status().unwrap_or(&StatusCode::NotFound);
        let first_line = format!(
            "HTTP/1.1 {} {}\r\n",
            *status as u16,
            status.canonical_reason()
        );
        log::trace!("sending: {}", &first_line);
        self.rw.write_all(first_line.as_bytes()).await?;

        self.finalize_headers();
        let mut headers = self.response_headers.iter().collect::<Vec<_>>();
        headers.sort_unstable_by_key(|(h, _)| h.as_str());

        for (header, values) in headers {
            for value in values.iter() {
                log::trace!("sending: {}: {}", &header, &value);

                self.rw
                    .write_all(format!("{}: {}\r\n", header, value).as_bytes())
                    .await?;
            }
        }

        self.rw.write_all(b"\r\n").await?;

        Ok(())
    }

    pub fn map_transport<T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static>(
        self,
        f: impl Fn(RW) -> T,
    ) -> Conn<T> {
        let Conn {
            request_headers,
            response_headers,
            path,
            status,
            version,
            state,
            rw,
            buffer,
            request_body_state,
            secure,
            method,
            response_body,
            stopper,
        } = self;

        Conn {
            request_headers,
            response_headers,
            method,
            response_body,
            path,
            status,
            version,
            state,
            rw: f(rw),
            buffer,
            request_body_state,
            secure,
            stopper,
        }
    }
}

pub fn utf8(d: &[u8]) -> &str {
    std::str::from_utf8(d).unwrap_or("not utf8")
}
