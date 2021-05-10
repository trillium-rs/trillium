use encoding_rs::Encoding;
use futures_lite::io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use http_types::headers::{CONTENT_LENGTH, CONTENT_TYPE, UPGRADE};
use http_types::transfer::Encoding::Chunked;
use http_types::{
    content::ContentLength,
    headers::{Header, Headers, DATE, EXPECT, TRANSFER_ENCODING},
    other::Date,
    transfer::TransferEncoding,
    Body, Extensions, Method, StatusCode, Version,
};
use httparse::{Request, EMPTY_HEADER};
use memmem::{Searcher, TwoWaySearcher};
use std::future::Future;
use std::iter;
use std::{
    convert::TryInto,
    fmt::{self, Debug, Formatter},
};

use crate::{
    body_encoder::BodyEncoder, received_body::ReceivedBodyState, util::encoding, ConnectionStatus,
    Error, ReceivedBody, Result, Stopper, Upgrade,
};

const MAX_HEADERS: usize = 128;
const MAX_HEAD_LENGTH: usize = 8 * 1024;
const SERVER: &str = concat!("trillium/", env!("CARGO_PKG_VERSION"));

/** A http connection

Unlike in other rust http implementations, this struct represents both
the request and the response, and holds the transport over which the
response will be sent.
*/
pub struct Conn<Transport> {
    pub(crate) request_headers: Headers,
    pub(crate) response_headers: Headers,
    pub(crate) path: String,
    pub(crate) method: Method,
    pub(crate) status: Option<StatusCode>,
    pub(crate) version: Version,
    pub(crate) state: Extensions,
    pub(crate) response_body: Option<Body>,
    pub(crate) transport: Transport,
    pub(crate) buffer: Option<Vec<u8>>,
    pub(crate) request_body_state: ReceivedBodyState,
    pub(crate) secure: bool,
    pub(crate) stopper: Stopper,
}

impl<Transport> Debug for Conn<Transport> {
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

impl<Transport> Conn<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /**
    read any number of new `Conn`s from the transport and call the
    provided handler function until either the connection is closed or
    an upgrade is requested. A return value of Ok(None) indicates a
    closed connection, while a return value of Ok(Some(upgrade))
    represents an upgrade.

    See the documentation for [`Conn`] for a full example.
    */

    pub async fn map<F, Fut>(
        transport: Transport,
        stopper: Stopper,
        handler: F,
    ) -> Result<Option<Upgrade<Transport>>>
    where
        F: Fn(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>> + Send,
    {
        let mut conn = Conn::new(transport, None, stopper).await?;

        loop {
            conn = match handler(conn).await.send().await? {
                ConnectionStatus::Upgrade(upgrade) => return Ok(Some(upgrade)),
                ConnectionStatus::Close => return Ok(None),
                ConnectionStatus::Conn(next) => next,
            }
        }
    }

    async fn send(mut self) -> Result<ConnectionStatus<Transport>> {
        self.send_headers().await?;

        if self.method() != &Method::Head {
            if let Some(body) = self.response_body.take() {
                io::copy(BodyEncoder::new(body), &mut self.transport).await?;
            }
        }

        self.finish().await
    }

    /// returns a read-only reference to the [state
    /// typemap](Extensions) for this conn
    ///
    /// stability note: this is not unlikely to be removed at some
    /// point, as this may end up being more of a trillium concern
    /// than a trillium_http concern
    pub fn state(&self) -> &Extensions {
        &self.state
    }

    /// returns a mutable reference to the [state
    /// typemap](Extensions) for this conn
    ///
    /// stability note: this is not unlikely to be removed at some
    /// point, as this may end up being more of a trillium concern
    /// than a trillium_http concern
    pub fn state_mut(&mut self) -> &mut Extensions {
        &mut self.state
    }

    /// returns an immutable reference to the request headers. it is
    /// not currently possible to mutate request headers for a conn
    /// that has been read from a transport. For synthetic conns, use
    /// [Conn<Synthetic>::request_headers_mut](Conn<trillium_http::Synthetic>::request_headers_mut)
    pub fn request_headers(&self) -> &Headers {
        &self.request_headers
    }

    /// returns a mutable reference to the response [headers](Headers)
    pub fn response_headers(&mut self) -> &mut Headers {
        &mut self.response_headers
    }

    /** sets the http status code from any `TryInto<StatusCode>`.

    Note: This currently will set the s
    ```
    # use trillium_http::{Conn, http_types::{Method, StatusCode}};
    # let mut conn = Conn::new_synthetic(Method::Get, "/", None);
    assert!(conn.status().is_none());

    conn.set_status(200); // a status can be set as a u16
    assert_eq!(conn.status().unwrap(), &StatusCode::Ok);

    conn.set_status(StatusCode::ImATeapot); // or as a StatusCode
    assert_eq!(conn.status().unwrap(), &StatusCode::ImATeapot);
    ```
    */
    pub fn set_status(&mut self, status: impl TryInto<StatusCode>) {
        self.status = Some(status.try_into().unwrap_or_else(|_| {
            log::error!("attempted to set an invalid status code");
            StatusCode::InternalServerError
        }));
    }

    /// retrieves the current response status code for this conn, if
    /// it has been set. See [Conn::set_status] for example usage.
    pub fn status(&self) -> Option<&StatusCode> {
        self.status.as_ref()
    }

    /**
    retrieves the path part of the request url, up to and excluding any query component
    ```
    # use trillium_http::{Conn, http_types::Method};
    let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", None);
    assert_eq!(conn.path(), "/some/path");
    ```
    */
    pub fn path(&self) -> &str {
        self.path.split('?').next().unwrap()
    }

    // pub fn url(&self) -> Result<Url> {
    //     let path = self.path();
    //     let host = self.host().unwrap_or_else(|| String::from("_"));
    //     let method = self.method();
    //     if path.starts_with("http://") || path.starts_with("https://") {
    //         Ok(Url::parse(path)?)
    //     } else if path.starts_with('/') {
    //         Ok(Url::parse(&format!("http://{}{}", host, path))?)
    //     } else if method == &Method::Connect {
    //         Ok(Url::parse(&format!("http://{}/", path))?)
    //     } else {
    //         Err(Error::UnexpectedUriFormat)
    //     }
    // }

    /**
    Sets the response body to anything that is [`impl Into<Body>`][Body].

    ```
    # use trillium_http::{Conn, http_types::{Method, Body}};
    # let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", None);
    conn.set_response_body("hello");
    conn.set_response_body(String::from("hello"));
    conn.set_response_body(vec![99, 97, 116]);
    ```
    */
    pub fn set_response_body(&mut self, body: impl Into<Body>) {
        let body = body.into();

        if self.response_headers.get(CONTENT_TYPE).is_none() {
            self.response_headers
                .insert(CONTENT_TYPE, body.mime().clone());
        }

        self.response_body = Some(body);
    }

    /// returns a reference to the current response body, if it has been set
    pub fn response_body(&self) -> Option<&Body> {
        self.response_body.as_ref()
    }

    /**
    remove the response body from this conn and return it

    ```
    # use trillium_http::{Conn, http_types::{Method, Body}};
    # let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", None);
    assert!(conn.response_body().is_none());
    conn.set_response_body("hello");
    assert!(conn.response_body().is_some());
    let body = conn.take_response_body();
    assert!(body.is_some());
    assert!(conn.response_body().is_none());
    ```
    */
    pub fn take_response_body(&mut self) -> Option<Body> {
        self.response_body.take()
    }

    /**
    returns the http method for this conn's request.
    ```
    # use trillium_http::{Conn, http_types::{Method, Body}};
    let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", None);
    assert_eq!(conn.method(), &Method::Get);
    ```
     */
    pub fn method(&self) -> &Method {
        &self.method
    }

    fn needs_100_continue(&self) -> bool {
        self.request_body_state == ReceivedBodyState::Start
            && self
                .request_headers
                .contains_ignore_ascii_case(EXPECT, "100-continue")
    }

    fn build_request_body(&mut self) -> ReceivedBody<'_, Transport> {
        ReceivedBody::new(
            self.request_content_length().ok().flatten(),
            &mut self.buffer,
            &mut self.transport,
            &mut self.request_body_state,
            None,
            encoding(&self.request_headers),
        )
    }

    /**
    returns the [encoding_rs::Encoding] for this request, as
    determined from the mime-type charset, if available

    ```
    # use trillium_http::{Conn, http_types::{Method, Body}};
    let mut conn = Conn::new_synthetic(Method::Get, "/", None);
    assert_eq!(conn.request_encoding(), encoding_rs::WINDOWS_1252); // the default
    conn.request_headers_mut().insert("content-type", "text/plain;charset=utf-16");
    assert_eq!(conn.request_encoding(), encoding_rs::UTF_16LE);
    ```
    */
    pub fn request_encoding(&self) -> &'static Encoding {
        encoding(&self.request_headers)
    }

    /**
    returns the [encoding_rs::Encoding] for this response, as
    determined from the mime-type charset, if available

    ```
    # use trillium_http::{Conn, http_types::{Method, Body}};
    let mut conn = Conn::new_synthetic(Method::Get, "/", None);
    assert_eq!(conn.response_encoding(), encoding_rs::WINDOWS_1252); // the default
    conn.response_headers().insert("content-type", "text/plain;charset=utf-16");
    assert_eq!(conn.response_encoding(), encoding_rs::UTF_16LE);
    ```
    */
    pub fn response_encoding(&self) -> &'static Encoding {
        encoding(&self.response_headers)
    }

    /**
    returns a [ReceivedBody] that references this conn. the conn
    retains all data and holds the singular transport, but the
    ReceivedBody provides an interface to read body content
    ```
    async_io::block_on(async {
    # use trillium_http::{Conn, http_types::{Method, Body}};
    let mut conn = Conn::new_synthetic(Method::Get, "/", Some(b"hello"));
    let request_body = conn.request_body().await;
    assert_eq!(request_body.content_length(), Some(5));
    assert_eq!(request_body.read_string().await.unwrap(), "hello");
    });
    ```
    */
    pub async fn request_body(&mut self) -> ReceivedBody<'_, Transport> {
        if self.needs_100_continue() {
            self.send_100_continue().await.ok();
        }

        self.build_request_body()
    }

    /// returns a clone of the [`stopper::Stopper`] for this Conn. use
    /// this to gracefully stop long-running futures and streams
    /// inside of handler functions
    pub fn stopper(&self) -> Stopper {
        self.stopper.clone()
    }

    fn validate_headers(request_headers: &Headers) -> Result<()> {
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

    /// Create a new Conn from the provided transport, as well as any
    /// bytes that have already been read from the transport, and a
    /// Stopper instance that will be used to signal graceful
    /// shutdown.
    pub async fn new(
        transport: Transport,
        bytes: Option<Vec<u8>>,
        stopper: Stopper,
    ) -> Result<Self> {
        let (transport, buf, extra_bytes) = Self::head(transport, bytes, &stopper).await?;
        let buffer = if extra_bytes.is_empty() {
            None
        } else {
            Some(extra_bytes)
        };
        let mut headers = [EMPTY_HEADER; MAX_HEADERS];
        let mut httparse_req = Request::new(&mut headers);
        let status = httparse_req.parse(&buf[..])?;
        if status.is_partial() {
            log::debug!("partial head content: {}", String::from_utf8_lossy(&buf));
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
            transport,
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

    /// predicate function to indicate whether the connection is
    /// secure. note that this does not necessarily indicate that the
    /// transport itself is secure, as it may indicate that
    /// trillium_http is behind a trusted reverse proxy that has
    /// terminated tls and provided appropriate headers to indicate
    /// this.
    pub fn is_secure(&self) -> bool {
        self.secure
    }

    async fn send_100_continue(&mut self) -> Result<()> {
        log::trace!("sending 100-continue");
        Ok(self
            .transport
            .write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
            .await?)
    }

    async fn head(
        mut transport: Transport,
        bytes: Option<Vec<u8>>,
        stopper: &Stopper,
    ) -> Result<(Transport, Vec<u8>, Vec<u8>)> {
        let mut buf = bytes.unwrap_or_default();
        let mut len = 0;

        let searcher = TwoWaySearcher::new(b"\r\n\r\n");
        loop {
            buf.extend(iter::repeat(0).take(100));
            let bytes = if len == 0 {
                stopper
                    .stop_future(transport.read(&mut buf[len..]))
                    .await
                    .ok_or(Error::Closed)??
            } else {
                transport.read(&mut buf[len..]).await?
            };

            let search_start = len.max(3) - 3;
            let search = searcher.search_in(&buf[search_start..]);

            if let Some(index) = search {
                buf.truncate(len + bytes);
                log::trace!(
                    "in head, finished headers:\n {}",
                    String::from_utf8_lossy(&buf[..search_start + index])
                );
                let body = buf.split_off(search_start + index + 4);
                if !body.is_empty() {
                    log::trace!(
                        "read the front of the body: {}",
                        String::from_utf8_lossy(&body)
                    );
                }
                return Ok((transport, buf, body));
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

    // fn inner_mut(&mut self) -> &mut Transport {
    //     &mut self.transport
    // }

    async fn next(mut self) -> Result<Self> {
        if !self.needs_100_continue() || self.request_body_state != ReceivedBodyState::Start {
            self.build_request_body().drain().await?;
        }
        Conn::new(self.transport, self.buffer, self.stopper).await
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
        self.transport.write_all(first_line.as_bytes()).await?;

        self.finalize_headers();
        let mut headers = self.response_headers.iter().collect::<Vec<_>>();
        headers.sort_unstable_by_key(|(h, _)| h.as_str());

        for (header, values) in headers {
            for value in values.iter() {
                log::trace!("sending: {}: {}", &header, &value);

                self.transport
                    .write_all(format!("{}: {}\r\n", header, value).as_bytes())
                    .await?;
            }
        }

        self.transport.write_all(b"\r\n").await?;

        Ok(())
    }

    /// applies a mapping function from one transport to another. This
    /// is particularly useful for boxing the transport. unless you're
    /// sure this is what you're looking for, you probably don't want
    /// to be using this
    pub fn map_transport<T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static>(
        self,
        f: impl Fn(Transport) -> T,
    ) -> Conn<T> {
        let Conn {
            request_headers,
            response_headers,
            path,
            status,
            version,
            state,
            transport,
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
            transport: f(transport),
            buffer,
            request_body_state,
            secure,
            stopper,
        }
    }
}
