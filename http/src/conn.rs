use crate::{
    Body, Buffer, Headers,
    KnownHeaderName::Host,
    Method, ReceivedBody, ServerConfig, Status, Swansong, TypeSet, Version,
    after_send::{AfterSend, SendStatus},
    h3::H3Connection,
    liveness::{CancelOnDisconnect, LivenessFut},
    received_body::ReceivedBodyState,
    util::encoding,
};
use encoding_rs::Encoding;
use futures_lite::{
    future,
    io::{AsyncRead, AsyncWrite},
};
use std::{
    borrow::Cow,
    fmt::{self, Debug, Formatter},
    future::Future,
    net::IpAddr,
    pin::pin,
    str,
    sync::Arc,
    time::Instant,
};
mod h3;
mod implementation;

/// Default Server header
pub const SERVER: &str = concat!("trillium-http/", env!("CARGO_PKG_VERSION"));

/// A http connection
///
/// Unlike in other rust http implementations, this struct represents both
/// the request and the response, and holds the transport over which the
/// response will be sent.
#[derive(fieldwork::Fieldwork)]
pub struct Conn<Transport> {
    #[field(get)]
    /// the shared [`ServerConfig`]
    pub(crate) server_config: Arc<ServerConfig>,

    /// request [headers](Headers)
    #[field(get, get_mut)]
    pub(crate) request_headers: Headers,

    /// response [headers](Headers)
    #[field(get, get_mut)]
    pub(crate) response_headers: Headers,

    pub(crate) path: Cow<'static, str>,

    /// the http method for this conn's request
    ///
    /// ```
    /// # use trillium_http::{Conn, Method};
    /// let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", ());
    /// assert_eq!(conn.method(), Method::Get);
    /// ```
    #[field(get, set, copy)]
    pub(crate) method: Method,

    /// the http status for this conn, if set
    #[field(get, copy)]
    pub(crate) status: Option<Status>,

    #[field(get = http_version, copy)]
    /// the http version for this conn
    pub(crate) version: Version,

    /// the [state typemap](TypeSet) for this conn
    #[field(get, get_mut)]
    pub(crate) state: TypeSet,

    /// the response [body](Body)
    ///
    /// ```
    /// # use trillium_http::{Conn, Method, Body};
    /// # let mut conn = Conn::new_synthetic(Method::Get, "/some/path?and&a=query", ());
    /// conn.set_response_body("hello");
    /// conn.set_response_body(String::from("hello"));
    /// conn.set_response_body(vec![99, 97, 116]);
    /// ```
    #[field(get, set, into, option_set_some, take)]
    pub(crate) response_body: Option<Body>,

    /// the transport
    ///
    /// This should only be used to call your own custom methods on the transport that do not read
    /// or write any data. Calling any method that reads from or writes to the transport will
    /// disrupt the HTTP protocol. If you're looking to transition from HTTP to another protocol,
    /// use an HTTP upgrade.
    #[field(get, get_mut)]
    pub(crate) transport: Transport,

    pub(crate) buffer: Buffer,

    pub(crate) request_body_state: ReceivedBodyState,

    pub(crate) after_send: AfterSend,

    /// whether the connection is secure
    ///
    /// note that this does not necessarily indicate that the transport itself is secure, as it may
    /// indicate that `trillium_http` is behind a trusted reverse proxy that has terminated tls and
    /// provided appropriate headers to indicate this.
    #[field(get, set, rename_predicates)]
    pub(crate) secure: bool,

    /// The [`Instant`] that the first header bytes for this conn were
    /// received, before any processing or parsing has been performed.
    #[field(get, copy)]
    pub(crate) start_time: Instant,

    /// The IP Address for the connection, if available
    #[field(set, get, copy, into)]
    pub(crate) peer_ip: Option<IpAddr>,

    /// the :authority http/3 pseudo-header
    #[field(set, get, into)]
    pub(crate) authority: Option<Cow<'static, str>>,

    /// the :scheme http/3 pseudo-header
    #[field(set, get, into)]
    pub(crate) scheme: Option<Cow<'static, str>>,

    /// the [quic connection state](H3Connection) for this conn's transport
    #[field(get)]
    pub(crate) h3_connection: Option<Arc<H3Connection>>,

    /// the :protocol http/3 pseudo-header
    #[field(set, get, into)]
    pub(crate) protocol: Option<Cow<'static, str>>,
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
            .field("authority", &self.authority)
            .field("scheme", &self.scheme)
            .field("protocol", &self.protocol)
            .field("h3_connection", &self.h3_connection)
            .finish()
    }
}

impl<Transport> Conn<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Returns the shared state on this conn, if set
    pub fn shared_state(&self) -> &TypeSet {
        &self.server_config.shared_state
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
    pub fn set_status(&mut self, status: impl TryInto<Status>) -> &mut Self {
        self.status = Some(status.try_into().unwrap_or_else(|_| {
            log::error!("attempted to set an invalid status code");
            Status::InternalServerError
        }));
        self
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
    pub fn set_host(&mut self, host: String) -> &mut Self {
        self.request_headers.insert(Host, host);
        self
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
        self.h3_connection.as_ref().map_or_else(
            || self.server_config.swansong.clone(),
            |h| h.swansong().clone(),
        )
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
            authority: self.authority,
            scheme: self.scheme,
            h3_connection: self.h3_connection,
            protocol: self.protocol,
        }
    }

    /// whether this conn is suitable for an http upgrade to another protocol
    pub fn should_upgrade(&self) -> bool {
        (self.method() == Method::Connect && self.status == Some(Status::Ok))
            || self.status == Some(Status::SwitchingProtocols)
    }

    #[doc(hidden)]
    pub fn finalize_headers(&mut self) {
        if self.version == Version::Http3 {
            self.finalize_response_headers_h3();
        } else {
            self.finalize_response_headers_1x();
        }
    }
}
