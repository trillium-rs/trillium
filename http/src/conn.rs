use crate::{
    Body, BufWriter, Buffer, ConnectionStatus, Error, Headers, HttpConfig,
    KnownHeaderName::{Connection, ContentLength, Date, Expect, Host, Server, TransferEncoding},
    Method, ReceivedBody, Result, Status, Swansong, TypeSet, Upgrade, Version,
    after_send::{AfterSend, SendStatus},
    copy,
    http_config::DEFAULT_CONFIG,
    liveness::{CancelOnDisconnect, LivenessFut},
    received_body::ReceivedBodyState,
    util::encoding,
};
use encoding_rs::Encoding;
use futures_lite::{
    future,
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
};
use memchr::memmem::Finder;
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    net::IpAddr,
    pin::pin,
    str,
    sync::Arc,
    time::{Instant, SystemTime},
};

/// Default Server header
pub const SERVER: &str = concat!("trillium/", env!("CARGO_PKG_VERSION"));

/// A http connection
///
/// Unlike in other rust http implementations, this struct represents both
/// the request and the response, and holds the transport over which the
/// response will be sent.
pub struct Conn<Transport> {
    pub(crate) request_headers: Headers,
    pub(crate) response_headers: Headers,
    pub(crate) path: String,
    pub(crate) method: Method,
    pub(crate) status: Option<Status>,
    pub(crate) version: Version,
    pub(crate) state: TypeSet,
    pub(crate) response_body: Option<Body>,
    pub(crate) transport: Transport,
    pub(crate) buffer: Buffer,
    pub(crate) request_body_state: ReceivedBodyState,
    pub(crate) secure: bool,
    pub(crate) swansong: Swansong,
    pub(crate) after_send: AfterSend,
    pub(crate) start_time: Instant,
    pub(crate) peer_ip: Option<IpAddr>,
    pub(crate) http_config: HttpConfig,
    pub(crate) shared_state: Option<Arc<TypeSet>>,
}

impl<Transport> Debug for Conn<Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conn")
            .field("http_config", &self.http_config)
            .field("request_headers", &self.request_headers)
            .field("response_headers", &self.response_headers)
            .field("path", &self.path)
            .field("method", &self.method)
            .field("status", &self.status)
            .field("version", &self.version)
            .field("state", &self.state)
            .field("shared_state", &self.shared_state)
            .field("response_body", &self.response_body)
            .field("transport", &"..")
            .field("buffer", &"..")
            .field("request_body_state", &self.request_body_state)
            .field("secure", &self.secure)
            .field("swansong", &self.swansong)
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
    /// Provides a default [`HttpConfig`]
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
        swansong: Swansong,
        handler: F,
    ) -> Result<Option<Upgrade<Transport>>>
    where
        F: FnMut(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>>,
    {
        Self::map_with_config(DEFAULT_CONFIG, transport, swansong, handler).await
    }

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
    pub async fn map_with_config<F, Fut>(
        http_config: HttpConfig,
        transport: Transport,
        swansong: Swansong,
        handler: F,
    ) -> Result<Option<Upgrade<Transport>>>
    where
        F: FnMut(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>>,
    {
        Self::map_with_config_and_shared_state(http_config, transport, swansong, None, handler)
            .await
    }

    /// read any number of new `Conn`s from the transport and call the
    /// provided handler function until either the connection is closed or
    /// an upgrade is requested. A return value of Ok(None) indicates a
    /// closed connection, while a return value of Ok(Some(upgrade))
    /// represents an upgrade.
    ///
    /// The `shared_state` `Arc<Stateset>` is available provided to all Conns on this transport if
    /// provided.
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
    pub async fn map_with_config_and_shared_state<F, Fut>(
        http_config: HttpConfig,
        transport: Transport,
        swansong: Swansong,
        shared_state: Option<Arc<TypeSet>>,
        mut handler: F,
    ) -> Result<Option<Upgrade<Transport>>>
    where
        F: FnMut(Conn<Transport>) -> Fut,
        Fut: Future<Output = Conn<Transport>>,
    {
        let mut conn = Conn::new_internal(
            http_config,
            transport,
            Vec::with_capacity(http_config.request_buffer_initial_len).into(),
            swansong,
            shared_state,
        )
        .await?;

        loop {
            conn = match handler(conn).await.send().await? {
                ConnectionStatus::Upgrade(upgrade) => return Ok(Some(upgrade)),
                ConnectionStatus::Close => return Ok(None),
                ConnectionStatus::Conn(next) => next,
            }
        }
    }

    async fn send(mut self) -> Result<ConnectionStatus<Transport>> {
        let mut output_buffer = Vec::with_capacity(self.http_config.response_buffer_len);
        self.write_headers(&mut output_buffer)?;

        let mut bufwriter = BufWriter::new_with_buffer(output_buffer, &mut self.transport);

        if self.method != Method::Head
            && !matches!(self.status, Some(Status::NotModified | Status::NoContent))
        {
            if let Some(body) = self.response_body.take() {
                copy(body, &mut bufwriter, self.http_config.copy_loops_per_yield).await?;
            }
        }

        bufwriter.flush().await?;
        self.after_send.call(true.into());
        self.finish().await
    }

    /// returns a read-only reference to the [state
    /// typemap](TypeSet) for this conn
    ///
    /// stability note: this is not unlikely to be removed at some
    /// point, as this may end up being more of a trillium concern
    /// than a `trillium_http` concern
    pub fn state(&self) -> &TypeSet {
        &self.state
    }

    /// returns a mutable reference to the [state
    /// typemap](TypeSet) for this conn
    ///
    /// stability note: this is not unlikely to be removed at some
    /// point, as this may end up being more of a trillium concern
    /// than a `trillium_http` concern
    pub fn state_mut(&mut self) -> &mut TypeSet {
        &mut self.state
    }

    /// Returns the shared state on this conn, if set
    ///
    /// stability note: this is not unlikely to be removed at some
    /// point, as this may end up being more of a trillium concern
    /// than a `trillium_http` concern
    pub fn shared_state(&self) -> Option<&TypeSet> {
        self.shared_state.as_deref()
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

    /// sets the http status code from any `TryInto<Status>`.
    ///
    /// ```
    /// # use trillium_http::{Conn, Method, Status};
    /// # let mut conn = Conn::new_synthetic(Method::Get, "/", ());
    /// assert!(conn.status().is_none());
    ///
    /// conn.set_status(200); // a status can be set as a u16
    /// assert_eq!(conn.status().unwrap(), Status::Ok);
    ///
    /// conn.set_status(Status::ImATeapot); // or as a Status
    /// assert_eq!(conn.status().unwrap(), Status::ImATeapot);
    /// ```
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

    /// retrieves the path part of the request url, up to and excluding any query component
    /// ```
    /// # use trillium_http::{Conn, Method};
    /// let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", ());
    /// assert_eq!(conn.path(), "/some/path");
    /// ```
    pub fn path(&self) -> &str {
        match self.path.split_once('?') {
            Some((path, _)) => path,
            None => &self.path,
        }
    }

    /// retrieves the combined path and any query
    pub fn path_and_query(&self) -> &str {
        &self.path
    }

    /// retrieves the query component of the path
    /// ```
    /// # use trillium_http::{Conn, Method};
    /// let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", ());
    /// assert_eq!(conn.querystring(), "and&a=query");
    ///
    /// let mut conn = Conn::new_synthetic(Method::Get, "/some/path", ());
    /// assert_eq!(conn.querystring(), "");
    /// ```
    pub fn querystring(&self) -> &str {
        self.path
            .split_once('?')
            .map(|(_, query)| query)
            .unwrap_or_default()
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

    /// Sets the response body to anything that is [`impl Into<Body>`][Body].
    ///
    /// ```
    /// # use trillium_http::{Conn, Method, Body};
    /// # let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", ());
    /// conn.set_response_body("hello");
    /// conn.set_response_body(String::from("hello"));
    /// conn.set_response_body(vec![99, 97, 116]);
    /// ```
    pub fn set_response_body(&mut self, body: impl Into<Body>) {
        self.response_body = Some(body.into());
    }

    /// returns a reference to the current response body, if it has been set
    pub fn response_body(&self) -> Option<&Body> {
        self.response_body.as_ref()
    }

    /// remove the response body from this conn and return it
    ///
    /// ```
    /// # use trillium_http::{Conn, Method};
    /// # let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", ());
    /// assert!(conn.response_body().is_none());
    /// conn.set_response_body("hello");
    /// assert!(conn.response_body().is_some());
    /// let body = conn.take_response_body();
    /// assert!(body.is_some());
    /// assert!(conn.response_body().is_none());
    /// ```
    pub fn take_response_body(&mut self) -> Option<Body> {
        self.response_body.take()
    }

    /// returns the http method for this conn's request.
    /// ```
    /// # use trillium_http::{Conn, Method};
    /// let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", ());
    /// assert_eq!(conn.method(), Method::Get);
    /// ```
    pub fn method(&self) -> Method {
        self.method
    }

    /// overrides the http method for this conn
    pub fn set_method(&mut self, method: Method) {
        self.method = method;
    }

    /// returns the http version for this conn.
    pub fn http_version(&self) -> Version {
        self.version
    }

    /// Cancels and drops the future if reading from the transport results in an error or empty read
    ///
    /// The use of this method is not advised if your connected http client employs pipelining
    /// (rarely seen in the wild), as it will buffer an unbounded number of requests one byte at a
    /// time
    ///
    /// If the client disconnects from the conn's transport, this function will return None. If the
    /// future completes without disconnection, this future will return Some containing the output
    /// of the future.
    ///
    /// The use of this method is not advised if your connected http client employs pipelining
    /// (rarely seen in the wild), as it will buffer an unbounded number of requests
    ///
    /// Note that the inner future cannot borrow conn, so you will need to clone or take any
    /// information needed to execute the future prior to executing this method.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use futures_lite::{AsyncRead, AsyncWrite};
    /// # use trillium_http::{Conn, Method};
    /// async fn something_slow_and_cancel_safe() -> String {
    ///     String::from("this was not actually slow")
    /// }
    /// async fn handler<T>(mut conn: Conn<T>) -> Conn<T>
    /// where
    ///     T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
    /// {
    ///     let Some(returned_body) = conn
    ///         .cancel_on_disconnect(async { something_slow_and_cancel_safe().await })
    ///         .await
    ///     else {
    ///         return conn;
    ///     };
    ///     conn.set_response_body(returned_body);
    ///     conn.set_status(200);
    ///     conn
    /// }
    /// ```
    pub async fn cancel_on_disconnect<'a, Fut>(&'a mut self, fut: Fut) -> Option<Fut::Output>
    where
        Fut: Future + Send + 'a,
    {
        CancelOnDisconnect(self, pin!(fut)).await
    }

    /// Check if the transport is connected by attempting to read from the transport
    ///
    /// # Example
    ///
    /// This is best to use at appropriate points in a long-running handler, like:
    ///
    /// ```rust
    /// # use futures_lite::{AsyncRead, AsyncWrite};
    /// # use trillium_http::{Conn, Method};
    /// # async fn something_slow_but_not_cancel_safe() {}
    /// async fn handler<T>(mut conn: Conn<T>) -> Conn<T>
    /// where
    ///     T: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
    /// {
    ///     for _ in 0..100 {
    ///         if conn.is_disconnected().await {
    ///             return conn;
    ///         }
    ///         something_slow_but_not_cancel_safe().await;
    ///     }
    ///     conn.set_status(200);
    ///     conn
    /// }
    /// ```
    pub async fn is_disconnected(&mut self) -> bool {
        future::poll_once(LivenessFut::new(self)).await.is_some()
    }

    fn needs_100_continue(&self) -> bool {
        self.request_body_state == ReceivedBodyState::Start
            && self.version != Version::Http1_0
            && self
                .request_headers
                .eq_ignore_ascii_case(Expect, "100-continue")
    }

    #[allow(clippy::needless_borrow, clippy::needless_borrows_for_generic_args)]
    fn build_request_body(&mut self) -> ReceivedBody<'_, Transport> {
        ReceivedBody::new_with_config(
            self.request_content_length().ok().flatten(),
            &mut self.buffer,
            &mut self.transport,
            &mut self.request_body_state,
            None,
            encoding(&self.request_headers),
            &self.http_config,
        )
    }

    /// returns the [`encoding_rs::Encoding`] for this request, as
    /// determined from the mime-type charset, if available
    ///
    /// ```
    /// # use trillium_http::{Conn, Method};
    /// let mut conn = Conn::new_synthetic(Method::Get, "/", ());
    /// assert_eq!(conn.request_encoding(), encoding_rs::WINDOWS_1252); // the default
    /// conn.request_headers_mut()
    ///     .insert("content-type", "text/plain;charset=utf-16");
    /// assert_eq!(conn.request_encoding(), encoding_rs::UTF_16LE);
    /// ```
    pub fn request_encoding(&self) -> &'static Encoding {
        encoding(&self.request_headers)
    }

    /// returns the [`encoding_rs::Encoding`] for this response, as
    /// determined from the mime-type charset, if available
    ///
    /// ```
    /// # use trillium_http::{Conn, Method};
    /// let mut conn = Conn::new_synthetic(Method::Get, "/", ());
    /// assert_eq!(conn.response_encoding(), encoding_rs::WINDOWS_1252); // the default
    /// conn.response_headers_mut()
    ///     .insert("content-type", "text/plain;charset=utf-16");
    /// assert_eq!(conn.response_encoding(), encoding_rs::UTF_16LE);
    /// ```
    pub fn response_encoding(&self) -> &'static Encoding {
        encoding(&self.response_headers)
    }

    /// returns a [`ReceivedBody`] that references this conn. the conn
    /// retains all data and holds the singular transport, but the
    /// `ReceivedBody` provides an interface to read body content
    /// ```
    /// # async_io::block_on(async {
    /// # use trillium_http::{Conn, Method};
    /// let mut conn = Conn::new_synthetic(Method::Get, "/", "hello");
    /// let request_body = conn.request_body().await;
    /// assert_eq!(request_body.content_length(), Some(5));
    /// assert_eq!(request_body.read_string().await.unwrap(), "hello");
    /// # });
    /// ```
    pub async fn request_body(&mut self) -> ReceivedBody<'_, Transport> {
        if self.needs_100_continue() {
            self.send_100_continue().await.ok();
        }

        self.build_request_body()
    }

    /// returns a clone of the [`swansong::Swansong`] for this Conn. use
    /// this to gracefully stop long-running futures and streams
    /// inside of handler functions
    pub fn swansong(&self) -> Swansong {
        self.swansong.clone()
    }

    fn validate_headers(request_headers: &Headers) -> Result<()> {
        let content_length = request_headers.has_header(ContentLength);
        let transfer_encoding_chunked =
            request_headers.eq_ignore_ascii_case(TransferEncoding, "chunked");

        if content_length && transfer_encoding_chunked {
            Err(Error::UnexpectedHeader(ContentLength.into()))
        } else {
            Ok(())
        }
    }

    /// # Create a new `Conn`
    ///
    /// This function creates a new conn from the provided
    /// [`Transport`][crate::transport::Transport], as well as any
    /// bytes that have already been read from the transport, and a
    /// [`Swansong`] instance that will be used to signal graceful
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
    pub async fn new(transport: Transport, bytes: Vec<u8>, swansong: Swansong) -> Result<Self> {
        Self::new_internal(DEFAULT_CONFIG, transport, bytes.into(), swansong, None).await
    }

    #[cfg(not(feature = "parse"))]
    async fn new_internal(
        http_config: HttpConfig,
        mut transport: Transport,
        mut buffer: Buffer,
        swansong: Swansong,
        shared_state: Option<Arc<TypeSet>>,
    ) -> Result<Self> {
        use crate::{HeaderName, HeaderValue};
        use httparse::{EMPTY_HEADER, Request};
        use std::str::FromStr;

        let (head_size, start_time) =
            Self::head(&mut transport, &mut buffer, &swansong, &http_config).await?;

        let mut headers = vec![EMPTY_HEADER; http_config.max_headers];
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
            let header_name = HeaderName::from_str(header.name)?;
            let header_value = HeaderValue::from(header.value.to_owned());
            request_headers.append(header_name, header_value);
        }

        Self::validate_headers(&request_headers)?;

        let path = httparse_req
            .path
            .ok_or(Error::RequestPathMissing)?
            .to_owned();
        log::trace!("received:\n{method} {path} {version}\n{request_headers}");

        let mut response_headers = Headers::new();
        response_headers.insert(Server, SERVER);

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
            swansong,
            after_send: AfterSend::default(),
            start_time,
            peer_ip: None,
            http_config,
            shared_state,
        })
    }

    #[cfg(feature = "parse")]
    async fn new_internal(
        http_config: HttpConfig,
        mut transport: Transport,
        mut buffer: Buffer,
        swansong: Swansong,
        shared_state: Option<Arc<TypeSet>>,
    ) -> Result<Self> {
        let (head_size, start_time) =
            Self::head(&mut transport, &mut buffer, &swansong, &http_config).await?;

        let first_line_index = Finder::new(b"\r\n")
            .find(&buffer)
            .ok_or(Error::InvalidHead)?;

        let mut spaces = memchr::memchr_iter(b' ', &buffer[..first_line_index]);
        let first_space = spaces.next().ok_or(Error::MissingMethod)?;
        let method = Method::parse(&buffer[0..first_space])?;
        let second_space = spaces.next().ok_or(Error::RequestPathMissing)?;
        let path = str::from_utf8(&buffer[first_space + 1..second_space])
            .map_err(|_| Error::RequestPathMissing)?
            .to_string();
        if path.is_empty() {
            return Err(Error::InvalidHead);
        }
        let version = Version::parse(&buffer[second_space + 1..first_line_index])?;
        if !matches!(version, Version::Http1_1 | Version::Http1_0) {
            return Err(Error::UnsupportedVersion(version));
        }

        let request_headers = Headers::parse(&buffer[first_line_index + 2..head_size])?;

        Self::validate_headers(&request_headers)?;

        let mut response_headers = Headers::new();
        response_headers.insert(Server, SERVER);

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
            swansong,
            after_send: AfterSend::default(),
            start_time,
            peer_ip: None,
            http_config,
            shared_state,
        })
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

    /// calculates any auto-generated headers for this conn prior to sending it
    pub fn finalize_headers(&mut self) {
        if self.status == Some(Status::SwitchingProtocols) {
            return;
        }

        self.response_headers
            .try_insert_with(Date, || httpdate::fmt_http_date(SystemTime::now()));

        if !matches!(self.status, Some(Status::NotModified | Status::NoContent)) {
            let has_content_length = if let Some(len) = self.body_len() {
                self.response_headers.try_insert(ContentLength, len);
                true
            } else {
                self.response_headers.has_header(ContentLength)
            };

            if self.version == Version::Http1_1 && !has_content_length {
                self.response_headers.insert(TransferEncoding, "chunked");
            } else {
                self.response_headers.remove(TransferEncoding);
            }
        }

        if self.swansong.state().is_shutting_down() {
            self.response_headers.insert(Connection, "close");
        }
    }

    /// Registers a function to call after the http response has been
    /// completely transferred. Please note that this is a sync function
    /// and should be computationally lightweight. If your _application_
    /// needs additional async processing, use your runtime's task spawn
    /// within this hook.  If your _library_ needs additional async
    /// processing in an `after_send` hook, please open an issue. This hook
    /// is currently designed for simple instrumentation and logging, and
    /// should be thought of as equivalent to a Drop hook.
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
        transport: &mut Transport,
        buf: &mut Buffer,
        swansong: &Swansong,
        http_config: &HttpConfig,
    ) -> Result<(usize, Instant)> {
        let mut len = 0;
        let mut start_with_read = buf.is_empty();
        let mut instant = None;
        let finder = Finder::new(b"\r\n\r\n");
        loop {
            if len >= http_config.head_max_len {
                return Err(Error::HeadersTooLong);
            }

            let bytes = if start_with_read {
                buf.expand();
                if len == 0 {
                    swansong
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
        Conn::new_internal(
            self.http_config,
            self.transport,
            self.buffer,
            self.swansong,
            self.shared_state,
        )
        .await
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
        (self.method() == Method::Connect && self.status == Some(Status::Ok))
            || self.status == Some(Status::SwitchingProtocols)
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
                .map_err(|_| Error::InvalidHeaderValue(ContentLength.into()))
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

        for (name, values) in &self.response_headers {
            if name.is_valid() {
                for value in values {
                    if value.is_valid() {
                        write!(output_buffer, "{name}: ")?;
                        output_buffer.extend_from_slice(value.as_ref());
                        write!(output_buffer, "\r\n")?;
                    } else {
                        log::error!("skipping invalid header value {value:?} for header {name}");
                    }
                }
            } else {
                log::error!("skipping invalid header with name {name:?}");
            }
        }

        write!(output_buffer, "\r\n")?;
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
            swansong,
            after_send,
            start_time,
            peer_ip,
            http_config,
            shared_state,
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
            swansong,
            after_send,
            start_time,
            peer_ip,
            http_config,
            shared_state,
        }
    }

    /// Get a reference to the transport.
    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    /// Get a mutable reference to the transport.
    ///
    /// This should only be used to call your own custom methods on the transport that do not read
    /// or write any data. Calling any method that reads from or writes to the transport will
    /// disrupt the HTTP protocol. If you're looking to transition from HTTP to another protocol,
    /// use an HTTP upgrade.
    pub fn transport_mut(&mut self) -> &mut Transport {
        &mut self.transport
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
