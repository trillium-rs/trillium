use crate::{
    copy,
    received_body::ReceivedBodyState,
    util::encoding,
    Body, ConnectionStatus, Error, HeaderValues, Headers,
    KnownHeaderName::{Connection, ContentLength, Date, Expect, Host, Server, TransferEncoding},
    Method, ReceivedBody, Result, StateSet, Status, Stopper, Upgrade, Version,
};
use encoding_rs::Encoding;
use futures_lite::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use httparse::{Request, EMPTY_HEADER};
use memchr::memmem::Finder;
use std::{
    convert::TryInto,
    fmt::{self, Debug, Formatter},
    future::Future,
    iter,
    net::IpAddr,
    str::FromStr,
    time::{Instant, SystemTime},
};

const MAX_HEADERS: usize = 128;
const MAX_HEAD_LENGTH: usize = 8 * 1024;
const SERVER: &str = concat!("trillium/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SendStatus {
    Success,
    Failure,
}
impl From<bool> for SendStatus {
    fn from(success: bool) -> Self {
        if success {
            Self::Success
        } else {
            Self::Failure
        }
    }
}

impl SendStatus {
    pub fn is_success(self) -> bool {
        SendStatus::Success == self
    }
}

#[derive(Default)]
pub(crate) struct AfterSend(Option<Box<dyn FnOnce(SendStatus) + Send + Sync + 'static>>);

impl AfterSend {
    pub(crate) fn call(&mut self, send_status: SendStatus) {
        if let Some(after_send) = self.0.take() {
            after_send(send_status);
        }
    }

    pub(crate) fn append<F>(&mut self, after_send: F)
    where
        F: FnOnce(SendStatus) + Send + Sync + 'static,
    {
        self.0 = Some(match self.0.take() {
            Some(existing_after_send) => Box::new(move |ss| {
                existing_after_send(ss);
                after_send(ss);
            }),
            None => Box::new(after_send),
        });
    }
}

impl Drop for AfterSend {
    fn drop(&mut self) {
        self.call(SendStatus::Failure);
    }
}

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
    pub(crate) status: Option<Status>,
    pub(crate) version: Version,
    pub(crate) state: StateSet,
    pub(crate) response_body: Option<Body>,
    pub(crate) transport: Transport,
    pub(crate) buffer: Option<Vec<u8>>,
    pub(crate) request_body_state: ReceivedBodyState,
    pub(crate) secure: bool,
    pub(crate) stopper: Stopper,
    pub(crate) after_send: AfterSend,
    pub(crate) start_time: Instant,
    pub(crate) peer_ip: Option<IpAddr>,
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
            .field("state", &self.state)
            .field("response_body", &self.response_body)
            .field("transport", &"..")
            .field(
                "buffer",
                &self.buffer.as_deref().map(String::from_utf8_lossy),
            )
            .field("request_body_state", &self.request_body_state)
            .field("secure", &self.secure)
            .field("stopper", &self.stopper)
            .field("after_send", &"..")
            .field("start_time", &self.start_time)
            .field("peer_ip", &self.peer_ip)
            .finish()
    }
}

impl<Transport> Conn<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// read any number of new `Conn`s from the transport and call the
    /// provided handler function until either the connection is closed or
    /// an upgrade is requested. A return value of Ok(None) indicates a
    /// closed connection, while a return value of Ok(Some(upgrade))
    /// represents an upgrade.
    ///
    /// See the documentation for [`Conn`] for a full example.
    ///
    /// # Errors
    ///
    /// This will return an error variant if:
    ///
    /// * there is an io error when reading from the underlying transport
    /// * headers are too long
    /// * we are unable to parse some aspect of the request
    /// * the request is an unsupported http version
    /// * we cannot make sense of the headers, such as if there is a
    /// `content-length` header as well as a `transfer-encoding: chunked`
    /// header.

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

        if self.method() != Method::Head
            && !matches!(self.status, Some(Status::NotModified | Status::NoContent))
        {
            if let Some(body) = self.response_body.take() {
                copy(body, &mut self.transport).await?;
            }
        }

        let mut after_send = std::mem::take(&mut self.after_send);
        after_send.call(true.into());
        std::mem::forget(after_send);

        self.finish().await
    }

    /// returns a read-only reference to the [state
    /// typemap](StateSet) for this conn
    ///
    /// stability note: this is not unlikely to be removed at some
    /// point, as this may end up being more of a trillium concern
    /// than a `trillium_http` concern
    pub fn state(&self) -> &StateSet {
        &self.state
    }

    /// returns a mutable reference to the [state
    /// typemap](StateSet) for this conn
    ///
    /// stability note: this is not unlikely to be removed at some
    /// point, as this may end up being more of a trillium concern
    /// than a `trillium_http` concern
    pub fn state_mut(&mut self) -> &mut StateSet {
        &mut self.state
    }

    /// returns a reference to the request headers
    pub fn request_headers(&self) -> &Headers {
        &self.request_headers
    }

    /// returns a mutable reference to the response [headers](Headers)
    pub fn request_headers_mut(&mut self) -> &mut Headers {
        &mut self.request_headers
    }

    /// returns a mutable reference to the response [headers](Headers)
    pub fn response_headers_mut(&mut self) -> &mut Headers {
        &mut self.response_headers
    }

    /// returns a reference to the response [headers](Headers)
    pub fn response_headers(&self) -> &Headers {
        &self.response_headers
    }

    /** sets the http status code from any `TryInto<Status>`.

    ```
    # use trillium_http::{Conn, Method, Status};
    # let mut conn = Conn::new_synthetic(Method::Get, "/", ());
    assert!(conn.status().is_none());

    conn.set_status(200); // a status can be set as a u16
    assert_eq!(conn.status().unwrap(), Status::Ok);

    conn.set_status(Status::ImATeapot); // or as a Status
    assert_eq!(conn.status().unwrap(), Status::ImATeapot);
    ```
    */
    pub fn set_status(&mut self, status: impl TryInto<Status>) {
        self.status = Some(status.try_into().unwrap_or_else(|_| {
            log::error!("attempted to set an invalid status code");
            Status::InternalServerError
        }));
    }

    /// retrieves the current response status code for this conn, if
    /// it has been set. See [`Conn::set_status`] for example usage.
    pub fn status(&self) -> Option<Status> {
        self.status
    }

    /**
    retrieves the path part of the request url, up to and excluding any query component
    ```
    # use trillium_http::{Conn, Method};
    let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", ());
    assert_eq!(conn.path(), "/some/path");
    ```
    */
    pub fn path(&self) -> &str {
        match self.path.split_once('?') {
            Some((path, _)) => path,
            None => &self.path,
        }
    }

    /**
    retrieves the query component of the path
    ```
    # use trillium_http::{Conn, Method};
    let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", ());
    assert_eq!(conn.querystring(), "and&a=query");

    let mut conn = Conn::new_synthetic(Method::Get, "/some/path", ());
    assert_eq!(conn.querystring(), "");

    ```
    */
    pub fn querystring(&self) -> &str {
        match self.path.split_once('?') {
            Some((_, query)) => query,
            None => "",
        }
    }

    /// get the host for this conn, if it exists
    pub fn host(&self) -> Option<&str> {
        self.request_headers.get_str(Host)
    }

    /// set the host for this conn
    pub fn set_host(&mut self, host: String) {
        self.request_headers.insert(Host, host);
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
    # use trillium_http::{Conn, Method, Body};
    # let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", ());
    conn.set_response_body("hello");
    conn.set_response_body(String::from("hello"));
    conn.set_response_body(vec![99, 97, 116]);
    ```
    */
    pub fn set_response_body(&mut self, body: impl Into<Body>) {
        self.response_body = Some(body.into());
    }

    /// returns a reference to the current response body, if it has been set
    pub fn response_body(&self) -> Option<&Body> {
        self.response_body.as_ref()
    }

    /**
    remove the response body from this conn and return it

    ```
    # use trillium_http::{Conn, Method};
    # let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", ());
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
    # use trillium_http::{Conn, Method};
    let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", ());
    assert_eq!(conn.method(), Method::Get);
    ```
     */
    pub fn method(&self) -> Method {
        self.method
    }

    /**
    overrides the http method for this conn
     */
    pub fn set_method(&mut self, method: Method) {
        self.method = method;
    }

    /**
    returns the http version for this conn.
    */
    pub fn http_version(&self) -> Version {
        self.version
    }

    fn needs_100_continue(&self) -> bool {
        self.request_body_state == ReceivedBodyState::Start
            && self
                .request_headers
                .eq_ignore_ascii_case(Expect, "100-continue")
    }

    #[allow(clippy::needless_borrow)]
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
    # use trillium_http::{Conn, Method};
    let mut conn = Conn::new_synthetic(Method::Get, "/", ());
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
    # use trillium_http::{Conn, Method};
    let mut conn = Conn::new_synthetic(Method::Get, "/", ());
    assert_eq!(conn.response_encoding(), encoding_rs::WINDOWS_1252); // the default
    conn.response_headers_mut().insert("content-type", "text/plain;charset=utf-16");
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
    # async_io::block_on(async {
    # use trillium_http::{Conn, Method};
    let mut conn = Conn::new_synthetic(Method::Get, "/", "hello");
    let request_body = conn.request_body().await;
    assert_eq!(request_body.content_length(), Some(5));
    assert_eq!(request_body.read_string().await.unwrap(), "hello");
    # });
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
        let content_length = request_headers.has_header(ContentLength);
        let transfer_encoding_chunked =
            request_headers.eq_ignore_ascii_case(TransferEncoding, "chunked");

        if content_length && transfer_encoding_chunked {
            Err(Error::UnexpectedHeader("content-length"))
        } else {
            Ok(())
        }
    }

    /// # Create a new `Conn`
    ///
    /// This function creates a new conn from the provided
    /// [`Transport`][crate::transport::Transport], as well as any
    /// bytes that have already been read from the transport, and a
    /// [`Stopper`] instance that will be used to signal graceful
    /// shutdown.
    ///
    /// # Errors
    ///
    /// This will return an error variant if:
    ///
    /// * there is an io error when reading from the underlying transport
    /// * headers are too long
    /// * we are unable to parse some aspect of the request
    /// * the request is an unsupported http version
    /// * we cannot make sense of the headers, such as if there is a
    /// `content-length` header as well as a `transfer-encoding: chunked`
    /// header.
    pub async fn new(
        transport: Transport,
        bytes: Option<Vec<u8>>,
        stopper: Stopper,
    ) -> Result<Self> {
        let (transport, buf, extra_bytes, start_time) =
            Self::head(transport, bytes, &stopper).await?;

        let buffer = if extra_bytes.is_empty() {
            None
        } else {
            Some(extra_bytes)
        };

        let mut headers = [EMPTY_HEADER; MAX_HEADERS];
        let mut httparse_req = Request::new(&mut headers);

        let status = httparse_req.parse(&buf[..])?;
        if status.is_partial() {
            log::trace!("partial head content: {}", String::from_utf8_lossy(&buf));
            return Err(Error::PartialHead);
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
            Some(version) => return Err(Error::UnsupportedVersion(version)),
            None => return Err(Error::MissingVersion),
        };

        let mut request_headers = Headers::with_capacity(httparse_req.headers.len());
        for header in httparse_req.headers {
            let header_name = crate::HeaderName::from_str(header.name)?;
            let header_value = crate::HeaderValue::from(header.value.to_owned());
            request_headers.append(header_name, header_value);
        }

        Self::validate_headers(&request_headers)?;

        let path = httparse_req
            .path
            .ok_or(Error::RequestPathMissing)?
            .to_owned();
        log::debug!("received:\n{method} {path} {version}\n{request_headers}");

        let response_headers = Self::build_response_headers();

        Ok(Self {
            transport,
            request_headers,
            method,
            version,
            path,
            buffer,
            response_headers,
            status: None,
            state: StateSet::new(),
            response_body: None,
            request_body_state: ReceivedBodyState::Start,
            secure: false,
            stopper,
            after_send: AfterSend::default(),
            start_time,
            peer_ip: None,
        })
    }

    fn build_response_headers() -> Headers {
        [
            (
                Date,
                HeaderValues::from(httpdate::fmt_http_date(SystemTime::now())),
            ),
            (Server, HeaderValues::from(SERVER)),
        ]
        .into_iter()
        .collect()
    }

    /// predicate function to indicate whether the connection is
    /// secure. note that this does not necessarily indicate that the
    /// transport itself is secure, as it may indicate that
    /// `trillium_http` is behind a trusted reverse proxy that has
    /// terminated tls and provided appropriate headers to indicate
    /// this.
    pub fn is_secure(&self) -> bool {
        self.secure
    }

    /// set whether the connection should be considered secure. note
    /// that this does not necessarily indicate that the transport
    /// itself is secure, as it may indicate that `trillium_http` is
    /// behind a trusted reverse proxy that has terminated tls and
    /// provided appropriate headers to indicate this.
    pub fn set_secure(&mut self, secure: bool) {
        self.secure = secure;
    }

    /**
    calculates any auto-generated headers for this conn prior to sending it
    */
    pub fn finalize_headers(&mut self) {
        if self.status == Some(Status::SwitchingProtocols) {
            return;
        }

        if !matches!(self.status, Some(Status::NotModified | Status::NoContent)) {
            if let Some(len) = self.body_len() {
                self.response_headers
                    .try_insert(ContentLength, len.to_string());
            }

            if !self.response_headers.has_header(ContentLength) && self.version == Version::Http1_1
            {
                self.response_headers.insert(TransferEncoding, "chunked");
            } else {
                self.response_headers.remove(TransferEncoding);
            }
        }

        if self.stopper.is_stopped() {
            self.response_headers.insert(Connection, "close");
        } else if !self
            .request_headers
            .eq_ignore_ascii_case(Connection, "close")
            && self.version == Version::Http1_1
        {
            self.response_headers.try_insert(Connection, "keep-alive");
        }
    }

    /**
    Registers a function to call after the http response has been
    completely transferred. Please note that this is a sync function
    and should be computationally lightweight. If your _application_
    needs additional async processing, use your runtime's task spawn
    within this hook.  If your _library_ needs additional async
    processing in an after_send hook, please open an issue. This hook
    is currently designed for simple instrumentation and logging, and
    should be thought of as equivalent to a Drop hook.
    */
    pub fn after_send<F>(&mut self, after_send: F)
    where
        F: FnOnce(SendStatus) + Send + Sync + 'static,
    {
        self.after_send.append(after_send);
    }

    /// The [`Instant`] that the first header bytes for this conn were
    /// received, before any processing or parsing has been performed.
    pub fn start_time(&self) -> Instant {
        self.start_time
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
    ) -> Result<(Transport, Vec<u8>, Vec<u8>, Instant)> {
        let mut buf = bytes.unwrap_or_default();
        let mut len = 0;
        let mut start_with_read = buf.is_empty();
        let mut instant = None;
        let finder = Finder::new(b"\r\n\r\n");
        loop {
            let bytes = if start_with_read {
                buf.extend(iter::repeat(0).take(1024));
                if len == 0 {
                    stopper
                        .stop_future(transport.read(&mut buf[..]))
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
                log::trace!(
                    "received head:\n{}",
                    String::from_utf8_lossy(&buf[..search_start + index])
                );
                let body = buf.split_off(search_start + index + 4);
                if !body.is_empty() {
                    log::trace!(
                        "read the front of the body: {}",
                        String::from_utf8_lossy(&body)
                    );
                }
                return Ok((transport, buf, body, instant.unwrap()));
            }

            len += bytes;

            if bytes == 0 {
                return if len == 0 {
                    Err(Error::Closed)
                } else {
                    log::debug!(
                        "disconnect? partial head content: \n{:?}",
                        String::from_utf8_lossy(&buf[..])
                    );
                    Err(Error::PartialHead)
                };
            }

            if len >= MAX_HEAD_LENGTH {
                log::error!("headers were {len}, greater than {MAX_HEAD_LENGTH}");
                return Err(Error::HeadersTooLong);
            }
        }
    }

    async fn next(mut self) -> Result<Self> {
        if !self.needs_100_continue() || self.request_body_state != ReceivedBodyState::Start {
            self.build_request_body().drain().await?;
        }
        Conn::new(self.transport, self.buffer, self.stopper).await
    }

    fn should_close(&self) -> bool {
        let request_connection = self.request_headers.get_lower(Connection);
        let response_connection = self.response_headers.get_lower(Connection);

        match (
            request_connection.as_deref(),
            response_connection.as_deref(),
        ) {
            (Some("keep-alive"), Some("keep-alive")) => false,
            (Some("close"), _) | (_, Some("close")) => true,
            _ => self.version == Version::Http1_0,
        }
    }

    fn should_upgrade(&self) -> bool {
        self.status == Some(Status::SwitchingProtocols)
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
            .eq_ignore_ascii_case(TransferEncoding, "chunked")
        {
            Ok(None)
        } else if let Some(cl) = self.request_headers.get_str(ContentLength) {
            cl.parse()
                .map(Some)
                .map_err(|_| Error::MalformedHeader("content-length".into()))
        } else {
            Ok(Some(0))
        }
    }

    fn body_len(&self) -> Option<u64> {
        match self.response_body {
            Some(ref body) => body.len(),
            None => Some(0),
        }
    }

    async fn send_headers(&mut self) -> Result<()> {
        let status = self.status().unwrap_or(Status::NotFound);
        let first_line = format!(
            "{} {} {}\r\n",
            self.version,
            status as u16,
            status.canonical_reason()
        );
        self.transport.write_all(first_line.as_bytes()).await?;

        self.finalize_headers();

        log::debug!("response:\n{first_line}\n{}", &self.response_headers);

        for (header, values) in self.response_headers.iter() {
            for value in &**values {
                log::trace!("sending: {header}: {value}");

                self.transport
                    .write_all(format!("{header}: ").as_bytes())
                    .await?;
                self.transport.write_all(value.as_ref()).await?;
                self.transport.write_all(b"\r\n").await?;
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
            after_send,
            start_time,
            peer_ip,
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
            after_send,
            start_time,
            peer_ip,
        }
    }

    /// sets the remote ip address for this conn, if available.
    pub fn set_peer_ip(&mut self, peer_ip: Option<IpAddr>) {
        self.peer_ip = peer_ip;
    }

    /// retrieves the remote ip address for this conn, if available.
    pub fn peer_ip(&self) -> Option<IpAddr> {
        self.peer_ip
    }
}
