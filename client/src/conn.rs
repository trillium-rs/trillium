use crate::{Pool, ResponseBody, h3::H3ClientState, util::encoding};
use encoding_rs::Encoding;
use std::{borrow::Cow, net::SocketAddr, sync::Arc, time::Duration};
use trillium_http::{
    Body, Buffer, HeaderName, HeaderValues, Headers, HttpContext, Method, ReceivedBody,
    ReceivedBodyState, Status, TypeSet, Version,
};
use trillium_server_common::{
    ArcedConnector, Transport,
    url::{Origin, Url},
};

mod h1;
mod h3;
mod shared;
mod unexpected_status_error;

#[cfg(any(feature = "serde_json", feature = "sonic-rs"))]
pub use shared::ClientSerdeError;
pub use unexpected_status_error::UnexpectedStatusError;

/// a client connection, representing both an outbound http request and a
/// http response
#[must_use]
#[derive(fieldwork::Fieldwork)]
pub struct Conn {
    pub(crate) pool: Option<Pool<Origin, Box<dyn Transport>>>,
    pub(crate) h3: Option<H3ClientState>,
    pub(crate) buffer: Buffer,
    pub(crate) response_body_state: ReceivedBodyState,
    pub(crate) config: ArcedConnector,
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

    #[field(get, get_mut)]
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
    #[field(with = with_body, argument = body, set, into, take, option_set_some)]
    pub(crate) request_body: Option<Body>,

    /// the timeout for this conn
    ///
    /// this can also be set on the client with [`Client::set_timeout`](crate::Client::set_timeout)
    /// and [`Client::with_timeout`](crate::Client::with_timeout)
    #[field(with, set, get, get_mut, take, copy, option_set_some)]
    pub(crate) timeout: Option<Duration>,

    /// the http version for this conn
    ///
    /// prior to conn execution, this reflects the intended http version that will be sent, and
    /// after execution this reflects the server-indicated http version
    #[field(get, set, copy)]
    pub(crate) http_version: Version,

    /// the :authority pseudo-header, populated during h3 header finalization
    #[field(get)]
    pub(crate) authority: Option<Cow<'static, str>>,
    /// the :scheme pseudo-header, populated during h3 header finalization

    #[field(get)]
    pub(crate) scheme: Option<Cow<'static, str>>,

    /// the :path pseudo-header, populated during h3 header finalization
    #[field(get)]
    pub(crate) path: Option<Cow<'static, str>>,

    /// an explicit request target override, used only for `OPTIONS *` and `CONNECT host:port`
    ///
    /// When set and the method is OPTIONS or CONNECT, this value is used as the HTTP request
    /// target instead of deriving it from the url. For all other methods, this field is ignored.
    #[field(with, set, get, option_set_some, into)]
    pub(crate) request_target: Option<Cow<'static, str>>,

    /// trailers sent with the request body, populated after the body has been fully sent.
    ///
    /// Only present when the request body was constructed with [`Body::new_with_trailers`] and
    /// the body has been fully sent. For H3, this is populated after `send_h3_request`; for H1,
    /// after `send_body` with a chunked body.
    #[field(get)]
    pub(crate) request_trailers: Option<Headers>,

    /// trailers received with the response body, populated after the response body has been fully
    /// read.
    ///
    /// For H3, these are decoded from the trailing HEADERS frame. For H1, from chunked trailers
    /// (once H1 trailer receive is implemented).
    #[field(get)]
    pub(crate) response_trailers: Option<Headers>,
}

/// default http user-agent header
pub const USER_AGENT: &str = concat!("trillium-client/", env!("CARGO_PKG_VERSION"));

impl Conn {
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

    pub(crate) fn response_encoding(&self) -> &'static Encoding {
        encoding(&self.response_headers)
    }

    /// returns a [`ReceivedBody`] that borrows the connection inside this conn.
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
        ReceivedBody::new(
            self.response_content_length(),
            &mut self.buffer,
            self.transport.as_mut().unwrap(),
            &mut self.response_body_state,
            None,
            encoding(&self.response_headers),
        )
        .with_trailers(&mut self.response_trailers)
        .into()
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

    /// Returns this conn to the connection pool if it is keepalive, and
    /// closes it otherwise. This will happen asynchronously as a spawned
    /// task when the conn is dropped, but calling it explicitly allows
    /// you to block on it and control where it happens.
    pub async fn recycle(mut self) {
        if self.is_keep_alive() && self.transport.is_some() && self.pool.is_some() {
            self.finish_reading_body().await;
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
