use crate::{
    Client, ResponseBody,
    response_body::{CleanupContext, OverrideBody},
    util::encoding,
};
use std::{borrow::Cow, mem, net::SocketAddr, sync::Arc, time::Duration};
use trillium_http::{
    Body, Buffer, Error, HeaderName, HeaderValues, Headers, HttpContext, Method, ProtocolSession,
    ReceivedBody, ReceivedBodyState, Status, TypeSet, Version,
};
use trillium_server_common::{Transport, url::Url};

mod h1;
mod h2;
mod h3;
mod request_body_buffer;
mod shared;
mod unexpected_status_error;

pub(crate) use h2::H2Pooled;
#[cfg(any(feature = "serde_json", feature = "sonic-rs"))]
pub use shared::ClientSerdeError;
pub use unexpected_status_error::UnexpectedStatusError;

/// a client connection, representing both an outbound http request and a
/// http response
#[must_use]
#[derive(fieldwork::Fieldwork)]
pub struct Conn {
    pub(crate) protocol_session: ProtocolSession,
    /// QUIC-connection WebTransport dispatcher slot (lazy-init) and the QUIC connection
    /// itself, retained on extended-CONNECT-with-`:protocol = webtransport` requests so
    /// `into_webtransport` can install the router and hand the QUIC connection to the
    /// returned [`WebTransportConnection`][trillium_webtransport::WebTransportConnection].
    #[cfg(feature = "webtransport")]
    pub(crate) wt_pool_entry: Option<crate::h3::H3PoolEntry>,
    pub(crate) buffer: Buffer,
    pub(crate) response_body_state: ReceivedBodyState,
    pub(crate) headers_finalized: bool,
    pub(crate) max_head_length: usize,
    pub(crate) state: TypeSet,
    pub(crate) context: Arc<HttpContext>,

    /// the transport for this conn
    ///
    /// This should only be used to call your own custom methods on the transport that do not read
    /// or write any data. Calling any method that reads from or writes to the transport will
    /// disrupt the HTTP protocol.
    #[field(get, get_mut)]
    pub(crate) transport: Option<Box<dyn Transport>>,

    /// the url for this conn.
    ///
    /// ```
    /// use trillium_client::{Client, Method};
    /// use trillium_testing::client_config;
    ///
    /// let client = Client::from(client_config());
    ///
    /// let conn = client.get("http://localhost:9080");
    ///
    /// let url = conn.url(); //<-
    ///
    /// assert_eq!(url.host_str().unwrap(), "localhost");
    /// ```
    #[field(get, set, get_mut)]
    pub(crate) url: Url,

    /// the method for this conn.
    ///
    /// ```
    /// use trillium_client::{Client, Method};
    /// use trillium_testing::client_config;
    ///
    /// let client = Client::from(client_config());
    /// let conn = client.get("http://localhost:9080");
    ///
    /// let method = conn.method(); //<-
    ///
    /// assert_eq!(method, Method::Get);
    /// ```
    #[field(get, set, copy)]
    pub(crate) method: Method,

    /// the request headers
    #[field(get, get_mut)]
    pub(crate) request_headers: Headers,

    #[field(get)]
    /// the response headers
    pub(crate) response_headers: Headers,

    /// the status code for this conn.
    ///
    /// If the conn has not yet been sent, this will be None.
    ///
    /// ```
    /// use trillium_client::{Client, Status};
    /// use trillium_testing::{client_config, with_server};
    ///
    /// async fn handler(conn: trillium::Conn) -> trillium::Conn {
    ///     conn.with_status(418)
    /// }
    ///
    /// with_server(handler, |url| async move {
    ///     let client = Client::new(client_config());
    ///     let conn = client.get(url).await?;
    ///     assert_eq!(Status::ImATeapot, conn.status().unwrap());
    ///     Ok(())
    /// });
    /// ```
    #[field(get, copy)]
    pub(crate) status: Option<Status>,

    /// the request body
    ///
    /// ```
    /// env_logger::init();
    /// use trillium_client::Client;
    /// use trillium_testing::{client_config, with_server};
    ///
    /// let handler = |mut conn: trillium::Conn| async move {
    ///     let body = conn.request_body_string().await.unwrap();
    ///     conn.ok(format!("request body was: {}", body))
    /// };
    ///
    /// with_server(handler, |url| async move {
    ///     let client = Client::from(client_config());
    ///     let mut conn = client
    ///         .post(url)
    ///         .with_body("body") //<-
    ///         .await?;
    ///
    ///     assert_eq!(
    ///         conn.response_body().read_string().await?,
    ///         "request body was: body"
    ///     );
    ///     Ok(())
    /// });
    /// ```
    #[field(get, with = with_body, argument = body, set, into, take, option_set_some)]
    pub(crate) request_body: Option<Body>,

    /// Whether the request body was fully buffered before sending (see
    /// [`request_body_buffer`](crate::conn::request_body_buffer)). When true, the h1 send path
    /// skips the `Expect: 100-continue` handshake — a buffered body is cheap to send in one shot.
    pub(crate) request_body_fully_buffered: bool,

    /// the timeout for this conn
    ///
    /// this can also be set on the client with [`Client::set_timeout`](crate::Client::set_timeout)
    /// and [`Client::with_timeout`](crate::Client::with_timeout)
    #[field(with, set, get, get_mut, take, copy, option_set_some)]
    pub(crate) timeout: Option<Duration>,

    /// whether this conn is halted.
    ///
    /// When set to `true` before execution, the network round-trip is skipped — the conn is
    /// returned to the caller with whatever response state has been populated synthetically
    /// (status, headers, body). Used by client middleware to short-circuit on cache hits,
    /// mocked responses, or open circuit-breakers. Cleared on egress so the user's conn handle
    /// never observes residual halt state after the awaited conn returns.
    ///
    /// Driven via [`ConnExt`](crate::ConnExt) — `halt` / `set_halted` / `is_halted`.
    pub(crate) halted: bool,

    /// transport-level error from the round-trip, if any.
    ///
    /// When the network call fails (connect refused, TLS handshake error, malformed HTTP frame,
    /// timeout, etc.) the framework stashes the error here and runs the handler chain's
    /// [`after_response`](crate::ClientHandler::after_response) anyway. A handler that recovers
    /// (stale-if-error cache, retry-with-fallback) calls
    /// [`ConnExt::take_error`](crate::ConnExt::take_error) to clear the error
    /// and populates response state synthetically; if the error is still present after all
    /// handlers finish, it propagates as `Err` from the awaited conn.
    pub(crate) error: Option<Error>,

    /// An override response body installed by middleware via
    /// [`ConnExt::set_response_body`](crate::ConnExt::set_response_body) or
    /// [`ConnExt::with_response_body`](crate::ConnExt::with_response_body). When
    /// set, [`Conn::response_body`] returns a [`ResponseBody`] backed by this body instead of
    /// the transport.
    pub(crate) body_override: Option<Body>,

    /// the http version *hint* for this conn
    ///
    /// Pre-execution this is the prior-knowledge hint, not the version that will necessarily be
    /// on the wire. `None` means "no hint, use auto-discovery" (Alt-Svc h3, ALPN/pooled h2);
    /// any `Some(version)` pins the protocol and suppresses auto-discovery. Post-execution this
    /// is `Some(version)` reflecting the version the request was actually sent over.
    ///
    /// The public [`http_version`](Conn::http_version) accessor resolves `None` to
    /// [`Version::Http1_1`]. See the crate-level [Protocol selection][crate#protocol-selection]
    /// documentation for the full hint → behavior table.
    #[field(set, with, option_set_some)]
    pub(crate) http_version: Option<Version>,

    /// the :authority pseudo-header, populated during h2 or h3 header finalization
    #[field(get)]
    pub(crate) authority: Option<Cow<'static, str>>,
    /// the :scheme pseudo-header, populated during h2 or h3 header finalization

    #[field(get)]
    pub(crate) scheme: Option<Cow<'static, str>>,

    /// the :path pseudo-header, populated during h2 or h3 header finalization
    #[field(get)]
    pub(crate) path: Option<Cow<'static, str>>,

    /// an explicit request target override, used only for `OPTIONS *` and `CONNECT host:port`
    ///
    /// When set and the method is OPTIONS or CONNECT, this value is used as the HTTP request
    /// target instead of deriving it from the url. For all other methods, this field is ignored.
    #[field(with, set, get, option_set_some, into)]
    pub(crate) request_target: Option<Cow<'static, str>>,

    /// the `:protocol` pseudo-header for an extended-CONNECT bootstrap (RFC 8441 over h2,
    /// RFC 9220 over h3). Triggers the h2/h3 exec paths to send HEADERS without `END_STREAM`
    /// and leave the stream open as a bidirectional byte channel.
    ///
    /// Only meaningful when method is `CONNECT` and [`http_version`][Self::http_version] is
    /// `Http2` or `Http3`. h1 and prior-version requests ignore this field.
    #[field(get)]
    pub(crate) protocol: Option<Cow<'static, str>>,

    /// trailers sent with the request body, populated after the body has been fully sent.
    ///
    /// Only present when the request body was constructed with [`Body::new_with_trailers`] and
    /// the body has been fully sent.
    #[field(get)]
    pub(crate) request_trailers: Option<Headers>,

    /// trailers received with the response body, populated after the response body has been fully
    /// read.
    #[field(get)]
    pub(crate) response_trailers: Option<Headers>,

    /// the [`Client`] that built this conn.
    #[field(get)]
    pub(crate) client: Client,

    /// A queued follow-up conn installed by middleware via
    /// [`ConnExt::set_followup`](crate::ConnExt::set_followup).
    ///
    /// When `Some` after the handler chain's `after_response` has fully unwound, the
    /// [`IntoFuture`][std::future::IntoFuture] loop picks it up: the current conn's response
    /// body is recycled, then the follow-up is swapped in and runs another full
    /// `(run → network → after_response)` cycle. Used by re-issuing handlers
    /// (`FollowRedirects`, retry, auth-refresh) instead of recursing into a nested `.await`.
    pub(crate) followup: Option<Box<Conn>>,

    /// Whether this conn is armed for an upgrade. When set, the protocol drivers
    /// transmit only request headers and leave the outbound direction open. Armed via
    /// [`ConnExt::upgrade`](crate::ConnExt::upgrade).
    pub(crate) upgrade: bool,
}

/// default http user-agent header
pub const USER_AGENT: &str = concat!("trillium-client/", env!("CARGO_PKG_VERSION"));

impl Conn {
    /// the http version for this conn
    ///
    /// Pre-execution this resolves the version *hint* — the default (no hint) reports
    /// [`Version::Http1_1`], which means "use auto-discovery," not "force HTTP/1.1." Setting any
    /// explicit version via [`with_http_version`](Conn::with_http_version) pins the protocol and
    /// suppresses auto-discovery. Post-execution this reflects the version the request was actually
    /// sent over.
    ///
    /// See the crate-level [Protocol selection][crate#protocol-selection] documentation for the
    /// full hint → behavior table.
    #[must_use]
    pub fn http_version(&self) -> Version {
        self.http_version.unwrap_or(Version::Http1_1)
    }

    /// chainable setter for [`inserting`](Headers::insert) a request header
    ///
    /// ```
    /// use trillium_client::Client;
    /// use trillium_testing::{client_config, with_server};
    ///
    /// let handler = |conn: trillium::Conn| async move {
    ///     let header = conn
    ///         .request_headers()
    ///         .get_str("some-request-header")
    ///         .unwrap_or_default();
    ///     let response = format!("some-request-header was {}", header);
    ///     conn.ok(response)
    /// };
    ///
    /// with_server(handler, |url| async move {
    ///     let client = Client::new(client_config());
    ///     let mut conn = client
    ///         .get(url)
    ///         .with_request_header("some-request-header", "header-value") // <--
    ///         .await?;
    ///     assert_eq!(
    ///         conn.response_body().read_string().await?,
    ///         "some-request-header was header-value"
    ///     );
    ///     Ok(())
    /// })
    /// ```
    pub fn with_request_header(
        mut self,
        name: impl Into<HeaderName<'static>>,
        value: impl Into<HeaderValues>,
    ) -> Self {
        self.request_headers.insert(name, value);
        self
    }

    /// chainable setter for `extending` request headers
    ///
    /// ```
    /// use trillium_client::Client;
    /// use trillium_testing::{client_config, with_server};
    ///
    /// let handler = |conn: trillium::Conn| async move {
    ///     let header = conn
    ///         .request_headers()
    ///         .get_str("some-request-header")
    ///         .unwrap_or_default();
    ///     let response = format!("some-request-header was {}", header);
    ///     conn.ok(response)
    /// };
    ///
    /// with_server(handler, move |url| async move {
    ///     let client = Client::new(client_config());
    ///     let mut conn = client
    ///         .get(url)
    ///         .with_request_headers([
    ///             ("some-request-header", "header-value"),
    ///             ("some-other-req-header", "other-header-value"),
    ///         ])
    ///         .await?;
    ///
    ///     assert_eq!(
    ///         conn.response_body().read_string().await?,
    ///         "some-request-header was header-value"
    ///     );
    ///     Ok(())
    /// })
    /// ```
    pub fn with_request_headers<HN, HV, I>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = (HN, HV)> + Send,
        HN: Into<HeaderName<'static>>,
        HV: Into<HeaderValues>,
    {
        self.request_headers.extend(headers);
        self
    }

    /// Chainable method to remove a request header if present
    pub fn without_request_header(mut self, name: impl Into<HeaderName<'static>>) -> Self {
        self.request_headers.remove(name);
        self
    }

    /// chainable setter for json body. this requires the `serde_json` crate feature to be enabled.
    #[cfg(feature = "serde_json")]
    pub fn with_json_body(self, body: &impl serde::Serialize) -> serde_json::Result<Self> {
        use trillium_http::KnownHeaderName;

        Ok(self
            .with_body(serde_json::to_string(body)?)
            .with_request_header(KnownHeaderName::ContentType, "application/json"))
    }

    /// chainable setter for json body. this requires the `sonic-rs` crate feature to be enabled.
    #[cfg(feature = "sonic-rs")]
    pub fn with_json_body(self, body: &impl serde::Serialize) -> sonic_rs::Result<Self> {
        use trillium_http::KnownHeaderName;

        Ok(self
            .with_body(sonic_rs::to_string(body)?)
            .with_request_header(KnownHeaderName::ContentType, "application/json"))
    }

    /// returns a [`ResponseBody`](crate::ResponseBody) that borrows the connection inside this
    /// conn.
    /// ```
    /// use trillium_client::Client;
    /// use trillium_testing::{client_config, with_server};
    ///
    /// let handler = |mut conn: trillium::Conn| async move { conn.ok("hello from trillium") };
    ///
    /// with_server(handler, |url| async move {
    ///     let client = Client::from(client_config());
    ///     let mut conn = client.get(url).await?;
    ///
    ///     let response_body = conn.response_body(); //<-
    ///
    ///     assert_eq!(19, response_body.content_length().unwrap());
    ///     let string = response_body.read_string().await?;
    ///     assert_eq!("hello from trillium", string);
    ///     Ok(())
    /// });
    /// ```
    #[allow(clippy::needless_borrow, clippy::needless_borrows_for_generic_args)]
    pub fn response_body(&mut self) -> ResponseBody<'_> {
        let content_length = self.response_content_length();
        let encoding = encoding(&self.response_headers);
        if let Some(body) = self.body_override.as_mut() {
            OverrideBody::new(body, encoding, self.context.config()).into()
        } else {
            ReceivedBody::new(
                content_length,
                &mut self.buffer,
                self.transport.as_mut().unwrap(),
                &mut self.response_body_state,
                None,
                encoding,
            )
            .with_trailers(&mut self.response_trailers)
            .with_protocol_session(self.protocol_session.clone())
            .into()
        }
    }

    /// Attempt to deserialize the response body. Note that this consumes the body content.
    #[cfg(feature = "serde_json")]
    pub async fn response_json<T>(&mut self) -> Result<T, ClientSerdeError>
    where
        T: serde::de::DeserializeOwned,
    {
        let body = self.response_body().read_string().await?;
        Ok(serde_json::from_str(&body)?)
    }

    /// Attempt to deserialize the response body. Note that this consumes the body content.
    #[cfg(feature = "sonic-rs")]
    pub async fn response_json<T>(&mut self) -> Result<T, ClientSerdeError>
    where
        T: serde::de::DeserializeOwned,
    {
        let body = self.response_body().read_string().await?;
        Ok(sonic_rs::from_str(&body)?)
    }

    /// Returns the conn or an [`UnexpectedStatusError`] that contains the conn
    ///
    /// ```
    /// use trillium_client::{Client, Status};
    /// use trillium_testing::{client_config, with_server};
    ///
    /// with_server(Status::NotFound, |url| async move {
    ///     let client = Client::new(client_config());
    ///     assert_eq!(
    ///         client.get(url).await?.success().unwrap_err().to_string(),
    ///         "expected a success (2xx) status code, but got 404 Not Found"
    ///     );
    ///     Ok(())
    /// });
    ///
    /// with_server(Status::Ok, |url| async move {
    ///     let client = Client::new(client_config());
    ///     assert!(client.get(url).await?.success().is_ok());
    ///     Ok(())
    /// });
    /// ```
    pub fn success(self) -> Result<Self, UnexpectedStatusError> {
        match self.status() {
            Some(status) if status.is_success() => Ok(self),
            _ => Err(self.into()),
        }
    }

    /// Detach the response body as an owned, `'static` value.
    ///
    /// Returns `None` if there is no body to take — neither an override has been installed nor
    /// a transport-backed body is available. Subsequent calls return `None`. Callers who want
    /// to wrap-and-replace the body (e.g. tee through a cache) compose this with
    /// [`ConnExt::set_response_body`][crate::ConnExt::set_response_body]; the conn's
    /// body slot is empty between the two calls.
    ///
    /// For a transport-backed body, this moves the transport into the returned
    /// `ResponseBody<'static>`. Drop on that value drains-and-pools (keepalive) or closes
    /// (otherwise) the transport via a spawned task; [`ResponseBody::recycle`] is the
    /// `await`-able variant. For an override body, the inner [`Body`] is moved out and any
    /// leftover transport on the conn is recycled immediately.
    #[must_use]
    pub fn take_response_body(&mut self) -> Option<ResponseBody<'static>> {
        let encoding = encoding(&self.response_headers);
        if let Some(body) = self.body_override.take() {
            return Some(OverrideBody::new(body, encoding, self.context.config()).into());
        }

        let cleanup = self.build_cleanup_context();
        let received = self.take_received_body(false)?;
        Some(ResponseBody::received_owned(received, cleanup))
    }

    /// Build a [`CleanupContext`] capturing the runtime and (if keepalive + pool configured)
    /// the pool + origin to insert into. Single source of truth for "what should happen to
    /// this conn's transport when its body is released" — both the on_completion callback
    /// wired into the body and the [`ResponseBody::recycle`] / `Drop` paths consume clones
    /// of this same context, so the user-driven and Drop-driven release paths agree.
    fn build_cleanup_context(&self) -> CleanupContext {
        // Only pool a transport whose response head we actually received (`status.is_some()`): a
        // conn abandoned before the response — a timeout or transport error mid-request — has an
        // empty `response_headers`, which `is_keep_alive` would read as persistent and recycle a
        // half-spent connection into the pool, poisoning the next request that reuses it.
        let h1_pool_origin = if self.status.is_some()
            && self.is_keep_alive()
            && let Some(pool) = self.client.pool().cloned()
        {
            Some((pool, self.url.origin()))
        } else {
            None
        };

        CleanupContext {
            runtime: self.client.connector().runtime(),
            h1_pool_origin,
            h1_idle_timeout: self.client.h1_idle_timeout(),
        }
    }

    /// Detach the transport-backed receive side of this conn as an owned `ReceivedBody`.
    ///
    /// Returns `None` when no transport is attached.
    ///
    /// `cleanup: true` wires a spawn-on-End callback inside the body for callers that hand
    /// the body off without awaiting it (`From<Conn> for Body`). `cleanup: false` is for
    /// callers that drive the body to End themselves and release the transport inline in
    /// their own poll loop — `take_response_body` does this so callers get a "transport is
    /// settled when read_to_end returns Ok(0)" guarantee instead of racing a spawned task.
    pub(crate) fn take_received_body(
        &mut self,
        cleanup: bool,
    ) -> Option<ReceivedBody<'static, Box<dyn Transport>>> {
        let _ = self.finalize_headers();
        let transport = self.transport.take()?;

        let on_completion = cleanup.then(|| {
            let cleanup = self.build_cleanup_context();
            Box::new(move |transport| cleanup.handoff(transport))
                as Box<dyn FnOnce(Box<dyn Transport>) + Send + Sync + 'static>
        });

        Some(
            ReceivedBody::new(
                self.response_content_length(),
                mem::take(&mut self.buffer),
                transport,
                self.response_body_state,
                on_completion,
                encoding(&self.response_headers),
            )
            .with_protocol_session(self.protocol_session.clone()),
        )
    }

    /// Returns this conn to the connection pool if it is keepalive, and
    /// closes it otherwise. This will happen asynchronously as a spawned
    /// task when the conn is dropped, but calling it explicitly allows
    /// you to block on it and control where it happens.
    pub async fn recycle(mut self) {
        if let Some(rb) = self.take_response_body() {
            rb.recycle().await;
        }
    }

    /// attempts to retrieve the connected peer address
    pub fn peer_addr(&self) -> Option<SocketAddr> {
        self.transport
            .as_ref()
            .and_then(|t| t.peer_addr().ok().flatten())
    }

    /// add state to the client conn and return self
    pub fn with_state<T: Send + Sync + 'static>(mut self, state: T) -> Self {
        self.insert_state(state);
        self
    }

    /// add state to the client conn, returning any previously set state of this type
    pub fn insert_state<T: Send + Sync + 'static>(&mut self, state: T) -> Option<T> {
        self.state.insert(state)
    }

    /// borrow state
    pub fn state<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.state.get()
    }

    /// borrow state mutably
    pub fn state_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.state.get_mut()
    }

    /// take state
    pub fn take_state<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.state.take()
    }
}
