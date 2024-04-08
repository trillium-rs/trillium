use crate::{
    after_send::{AfterSend, SendStatus},
    liveness::{CancelOnDisconnect, LivenessFut},
    received_body::ReceivedBodyState,
    util::encoding,
    Body, Buffer, Headers,
    KnownHeaderName::{Connection, ContentLength, Date, Host, TransferEncoding},
    Method, ReceivedBody, ServerConfig, Status, Swansong, TypeSet, Version,
};
use encoding_rs::Encoding;
use futures_lite::{
    future,
    io::{AsyncRead, AsyncWrite},
};
use std::{
    fmt::{self, Debug, Formatter},
    future::Future,
    net::IpAddr,
    pin::pin,
    str,
    sync::Arc,
    time::{Instant, SystemTime},
};
mod implementation;

/// Default Server header
pub const SERVER: &str = concat!("trillium/", env!("CARGO_PKG_VERSION"));

/// A http connection
///
/// Unlike in other rust http implementations, this struct represents both
/// the request and the response, and holds the transport over which the
/// response will be sent.
pub struct Conn<Transport> {
    pub(crate) server_config: Arc<ServerConfig>,
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
    pub(crate) after_send: AfterSend,
    pub(crate) start_time: Instant,
    pub(crate) peer_ip: Option<IpAddr>,
}

impl<Transport> Debug for Conn<Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conn")
            .field("server_config", &self.server_config)
            .field("request_headers", &self.request_headers)
            .field("response_headers", &self.response_headers)
            .field("path", &self.path)
            .field("method", &self.method)
            .field("status", &self.status)
            .field("version", &self.version)
            .field("state", &self.state)
            .field("response_body", &self.response_body)
            .field("transport", &format_args!(".."))
            .field("buffer", &format_args!(".."))
            .field("request_body_state", &self.request_body_state)
            .field("secure", &self.secure)
            .field("after_send", &format_args!(".."))
            .field("start_time", &self.start_time)
            .field("peer_ip", &self.peer_ip)
            .finish()
    }
}

impl<Transport> Conn<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
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
    pub fn state_mut(&mut self) -> &mut TypeSet {
        &mut self.state
    }

    /// Returns the shared state on this conn, if set
    pub fn shared_state(&self) -> &TypeSet {
        &self.server_config.shared_state
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
        self.server_config.swansong.clone()
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

        if self.server_config.swansong.state().is_shutting_down() {
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

    /// applies a mapping function from one transport to another. This
    /// is particularly useful for boxing the transport. unless you're
    /// sure this is what you're looking for, you probably don't want
    /// to be using this
    pub fn map_transport<NewTransport>(
        self,
        f: impl Fn(Transport) -> NewTransport,
    ) -> Conn<NewTransport>
    where
        NewTransport: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
    {
        Conn {
            server_config: self.server_config,
            request_headers: self.request_headers,
            response_headers: self.response_headers,
            method: self.method,
            response_body: self.response_body,
            path: self.path,
            status: self.status,
            version: self.version,
            state: self.state,
            transport: f(self.transport),
            buffer: self.buffer,
            request_body_state: self.request_body_state,
            secure: self.secure,
            after_send: self.after_send,
            start_time: self.start_time,
            peer_ip: self.peer_ip,
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
