use crate::{
    Body, Buffer, Headers, HttpContext,
    KnownHeaderName::Host,
    Method, ProtocolSession, ReceivedBody, Status, Swansong, TypeSet, Version,
    after_send::{AfterSend, SendStatus},
    h2::H2Connection,
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
mod h1;
mod h2;
mod h3;
mod shared;
pub(crate) use h1::{HeadError, write_headers_or_trailers};
pub(crate) use h3::{H3FirstFrame, encode_field_section_h3};
pub(crate) use shared::ConnParts;

/// An HTTP connection.
///
/// This struct represents both the request and the response, and holds the
/// transport over which the response will be sent.
#[derive(fieldwork::Fieldwork)]
pub struct Conn<Transport> {
    #[field(get)]
    /// the shared [`HttpContext`]
    pub(crate) context: Arc<HttpContext>,

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

    /// The HTTP protocol version in use on this connection.
    ///
    /// ```
    /// # use trillium_http::{Conn, Method, Version};
    /// let conn = Conn::new_synthetic(Method::Get, "/", ());
    /// assert_eq!(conn.http_version(), Version::Http1_1);
    /// ```
    #[field(get = http_version, copy)]
    pub(crate) version: Version,

    /// the [state typemap](TypeSet) for this conn
    #[field(get, get_mut)]
    pub(crate) state: TypeSet,

    /// the response [body](Body)
    ///
    /// ```
    /// # use trillium_testing::HttpTest;
    /// HttpTest::new(|conn| async move { conn.with_response_body("hello") })
    ///     .get("/")
    ///     .block()
    ///     .assert_body("hello");
    ///
    /// HttpTest::new(|conn| async move { conn.with_response_body(String::from("world")) })
    ///     .get("/")
    ///     .block()
    ///     .assert_body("world");
    ///
    /// HttpTest::new(|conn| async move { conn.with_response_body(vec![99, 97, 116]) })
    ///     .get("/")
    ///     .block()
    ///     .assert_body("cat");
    /// ```
    #[field(get, set, into, option_set_some, take, with)]
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

    /// the `:authority` pseudo-header
    #[field(set, get, into)]
    pub(crate) authority: Option<Cow<'static, str>>,

    /// the `:scheme` pseudo-header
    #[field(set, get, into)]
    pub(crate) scheme: Option<Cow<'static, str>>,

    /// the [`ProtocolSession`] for this conn — the per-protocol session state
    /// (h2/h3 connection driver and stream id) bundled into a single enum so the
    /// "set together" invariant is enforced at the type level. `Http1` for
    /// h1 / synthetic conns.
    pub(crate) protocol_session: ProtocolSession,

    /// the `:protocol` pseudo-header (extended CONNECT)
    #[field(set, get, into)]
    pub(crate) protocol: Option<Cow<'static, str>>,

    /// request trailers, populated after the request body has been fully read
    #[field(get, get_mut)]
    pub(crate) request_trailers: Option<Headers>,

    /// Marker set via [`Conn::upgrade`].
    pub(crate) upgrade: bool,
}

impl<Transport> Debug for Conn<Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conn")
            .field("context", &self.context)
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
            .field("protocol_session", &self.protocol_session)
            .field("request_trailers", &self.request_trailers)
            .field("upgrade", &self.upgrade)
            .finish()
    }
}

impl<Transport> Conn<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
{
    /// Returns the shared state typemap for this conn.
    pub fn shared_state(&self) -> &TypeSet {
        &self.context.shared_state
    }

    /// sets the http status code from any `TryInto<Status>`.
    ///
    /// ```
    /// # use trillium_http::Status;
    /// # trillium_testing::HttpTest::new(|mut conn| async move {
    /// assert!(conn.status().is_none());
    ///
    /// conn.set_status(200); // a status can be set as a u16
    /// assert_eq!(conn.status().unwrap(), Status::Ok);
    ///
    /// conn.set_status(Status::ImATeapot); // or as a Status
    /// assert_eq!(conn.status().unwrap(), Status::ImATeapot);
    /// conn
    /// # }).get("/").block().assert_status(Status::ImATeapot);
    /// ```
    pub fn set_status(&mut self, status: impl TryInto<Status>) -> &mut Self {
        self.status = Some(status.try_into().unwrap_or_else(|_| {
            log::error!("attempted to set an invalid status code");
            Status::InternalServerError
        }));
        self
    }

    /// sets the http status code from any `TryInto<Status>`, returning Conn
    #[must_use]
    pub fn with_status(mut self, status: impl TryInto<Status>) -> Self {
        self.set_status(status);
        self
    }

    /// The status to send on the wire: the explicitly-set status, or a
    /// method-appropriate default when a handler left it unset. Unhandled
    /// requests default to `404 Not Found`, except CONNECT, which defaults to
    /// `501 Not Implemented`: an origin server implements no tunnel, and 404's
    /// resource model does not apply to CONNECT's authority-form target.
    pub(crate) fn response_status(&self) -> Status {
        self.status.unwrap_or(match self.method {
            Method::Connect => Status::NotImplemented,
            _ => Status::NotFound,
        })
    }

    /// retrieves the path part of the request url, up to and excluding any query component
    /// ```
    /// # use trillium_testing::HttpTest;
    /// HttpTest::new(|mut conn| async move {
    ///     assert_eq!(conn.path(), "/some/path");
    ///     conn.with_status(200)
    /// })
    /// .get("/some/path?and&a=query")
    /// .block()
    /// .assert_ok();
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

    /// retrieves the query component of the path, or an empty &str
    ///
    /// ```
    /// # use trillium_testing::HttpTest;
    /// let server = HttpTest::new(|conn| async move {
    ///     let querystring = conn.querystring().to_string();
    ///     conn.with_response_body(querystring).with_status(200)
    /// });
    ///
    /// server
    ///     .get("/some/path?and&a=query")
    ///     .block()
    ///     .assert_body("and&a=query");
    ///
    /// server.get("/some/path").block().assert_body("");
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
    ///     conn.with_response_body(returned_body).with_status(200)
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
    ///     conn.with_status(200)
    /// }
    /// ```
    pub async fn is_disconnected(&mut self) -> bool {
        future::poll_once(LivenessFut::new(self)).await.is_some()
    }

    /// returns the [`encoding_rs::Encoding`] for this request, as determined from the mime-type
    /// charset, if available
    ///
    /// ```
    /// # use trillium_testing::HttpTest;
    /// HttpTest::new(|mut conn| async move {
    ///     assert_eq!(conn.request_encoding(), encoding_rs::WINDOWS_1252); // the default
    ///
    ///     conn.request_headers_mut()
    ///         .insert("content-type", "text/plain;charset=utf-16");
    ///     assert_eq!(conn.request_encoding(), encoding_rs::UTF_16LE);
    ///
    ///     conn.with_status(200)
    /// })
    /// .get("/")
    /// .block()
    /// .assert_ok();
    /// ```
    pub fn request_encoding(&self) -> &'static Encoding {
        encoding(&self.request_headers)
    }

    /// returns the [`encoding_rs::Encoding`] for this response, as
    /// determined from the mime-type charset, if available
    ///
    /// ```
    /// # use trillium_testing::HttpTest;
    /// HttpTest::new(|mut conn| async move {
    ///     assert_eq!(conn.response_encoding(), encoding_rs::WINDOWS_1252); // the default
    ///     conn.response_headers_mut()
    ///         .insert("content-type", "text/plain;charset=utf-16");
    ///
    ///     assert_eq!(conn.response_encoding(), encoding_rs::UTF_16LE);
    ///
    ///     conn.with_status(200)
    /// })
    /// .get("/")
    /// .block()
    /// .assert_ok();
    /// ```
    pub fn response_encoding(&self) -> &'static Encoding {
        encoding(&self.response_headers)
    }

    /// returns a [`ReceivedBody`] that references this conn. the conn
    /// retains all data and holds the singular transport, but the
    /// `ReceivedBody` provides an interface to read body content.
    ///
    /// If the request included an `Expect: 100-continue` header, the 100 Continue response is sent
    /// lazily on the first read from the returned [`ReceivedBody`].
    /// ```
    /// # use trillium_testing::HttpTest;
    /// let server = HttpTest::new(|mut conn| async move {
    ///     let request_body = conn.request_body();
    ///     assert_eq!(request_body.content_length(), Some(5));
    ///     assert_eq!(request_body.read_string().await.unwrap(), "hello");
    ///     conn.with_status(200)
    /// });
    ///
    /// server.post("/").with_body("hello").block().assert_ok();
    /// ```
    pub fn request_body(&mut self) -> ReceivedBody<'_, Transport> {
        let needs_100_continue = self.needs_100_continue();
        let body = self.build_request_body();
        if needs_100_continue {
            body.with_send_100_continue()
        } else {
            body
        }
    }

    /// returns a clone of the [`swansong::Swansong`] for this Conn. use
    /// this to gracefully stop long-running futures and streams
    /// inside of handler functions
    pub fn swansong(&self) -> Swansong {
        self.protocol_session
            .h3_connection()
            .map_or_else(|| self.context.swansong.clone(), |h| h.swansong().clone())
    }

    /// Registers a function to call after the http response has been
    /// completely transferred.
    ///
    /// The callback is guaranteed to fire **exactly once** before the conn is
    /// dropped. Either the codec's send path invokes it with the real outcome,
    /// or — if the conn is dropped before send completes (handler panic,
    /// transport error, mid-write disconnect) — the drop fallback invokes it
    /// with a `SendStatus` whose `is_success()` returns false. Multiple
    /// registrations on the same conn chain in registration order.
    ///
    /// Because firing is ordered by send-completion rather than handler return,
    /// this is the right hook for instrumentation that wants to report what the
    /// peer actually observed.
    ///
    /// This is a sync function and should be computationally lightweight. If
    /// your _application_ needs additional async processing, use your runtime's
    /// task spawn within this hook. If your _library_ needs additional async
    /// processing in an `after_send` hook, please open an issue.
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
        // Manual respread: rustc treats `Conn<Transport>` and `Conn<NewTransport>` as
        // disjoint types and rejects `..self` without the unstable
        // `type_changing_struct_update` feature. If a new field is added to `Conn`,
        // update this respread, `Upgrade::map_transport`, and `From<Conn> for Upgrade`
        // (`upgrade.rs`) — they share this drift hazard.
        Conn {
            context: self.context,
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
            protocol: self.protocol,
            protocol_session: self.protocol_session,
            request_trailers: self.request_trailers,
            upgrade: self.upgrade,
        }
    }

    /// whether this conn is suitable for an http upgrade to another protocol
    pub fn should_upgrade(&self) -> bool {
        self.upgrade
            || (self.method() == Method::Connect && self.status == Some(Status::Ok))
            || self.status == Some(Status::SwitchingProtocols)
    }

    /// Mark this conn to be handed off as an upgrade once the response headers are sent.
    /// Set the response status (typically `200`) and any headers describing the upgraded
    /// byte stream before calling; the handler's `upgrade` method receives an [`Upgrade`]
    /// with per-protocol framing applied on its `AsyncRead`/`AsyncWrite`.
    #[doc(hidden)]
    #[must_use]
    pub fn upgrade(mut self) -> Self {
        self.upgrade = true;
        self
    }

    #[doc(hidden)]
    pub fn finalize_headers(&mut self) {
        if self.version == Version::Http3 {
            self.finalize_response_headers_h3();
        } else {
            self.finalize_response_headers_1x();
        }
    }

    /// the [`H2Connection`] driver for this conn, if this is an HTTP/2 request
    pub fn h2_connection(&self) -> Option<&Arc<H2Connection>> {
        self.protocol_session.h2_connection()
    }

    /// the h2 stream id for this conn, if this is an HTTP/2 request
    pub fn h2_stream_id(&self) -> Option<u32> {
        self.protocol_session.h2_stream_id()
    }

    /// the [`H3Connection`] driver for this conn, if this is an HTTP/3 request
    pub fn h3_connection(&self) -> Option<&Arc<H3Connection>> {
        self.protocol_session.h3_connection()
    }

    /// the h3 stream id for this conn, if this is an HTTP/3 request
    pub fn h3_stream_id(&self) -> Option<u64> {
        self.protocol_session.h3_stream_id()
    }
}
