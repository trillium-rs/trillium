use crate::{util::encoding, Pool};
use encoding_rs::Encoding;
use std::{net::SocketAddr, time::Duration};
use trillium_http::{
    transport::BoxedTransport, Body, Buffer, HeaderName, HeaderValues, Headers, Method,
    ReceivedBody, ReceivedBodyState, Status, TypeSet, Version,
};
use trillium_server_common::{
    url::{Origin, Url},
    ArcedConnector, Transport,
};

mod implementation;
mod unexpected_status_error;

pub use unexpected_status_error::UnexpectedStatusError;

/// A wrapper error for [`trillium_http::Error`] or
/// [`serde_json::Error`]. Only available when the `json` crate feature is
/// enabled.
#[cfg(feature = "json")]
#[derive(thiserror::Error, Debug)]
pub enum ClientSerdeError {
    /// A [`trillium_http::Error`]
    #[error(transparent)]
    HttpError(#[from] trillium_http::Error),

    /// A [`serde_json::Error`]
    #[error(transparent)]
    JsonError(#[from] serde_json::Error),
}

/// a client connection, representing both an outbound http request and a
/// http response
#[must_use]
pub struct Conn {
    pub(crate) url: Url,
    pub(crate) method: Method,
    pub(crate) request_headers: Headers,
    pub(crate) response_headers: Headers,
    pub(crate) transport: Option<BoxedTransport>,
    pub(crate) status: Option<Status>,
    pub(crate) request_body: Option<Body>,
    pub(crate) pool: Option<Pool<Origin, BoxedTransport>>,
    pub(crate) buffer: Buffer,
    pub(crate) response_body_state: ReceivedBodyState,
    pub(crate) config: ArcedConnector,
    pub(crate) headers_finalized: bool,
    pub(crate) timeout: Option<Duration>,
    pub(crate) http_version: Version,
    pub(crate) max_head_length: usize,
    pub(crate) state: TypeSet,
}

/// default http user-agent header
pub const USER_AGENT: &str = concat!("trillium-client/", env!("CARGO_PKG_VERSION"));

impl Conn {
    /// borrow the request headers
    pub fn request_headers(&self) -> &Headers {
        &self.request_headers
    }

    /// chainable setter for [`inserting`](Headers::insert) a request header
    ///
    /// ```
    /// use trillium_testing::client_config;
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
    /// let client = trillium_client::Client::new(client_config());
    ///
    /// trillium_testing::with_server(handler, |url| async move {
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
    /// let handler = |conn: trillium::Conn| async move {
    ///     let header = conn
    ///         .request_headers()
    ///         .get_str("some-request-header")
    ///         .unwrap_or_default();
    ///     let response = format!("some-request-header was {}", header);
    ///     conn.ok(response)
    /// };
    ///
    /// use trillium_testing::client_config;
    /// let client = trillium_client::client(client_config());
    ///
    /// trillium_testing::with_server(handler, move |url| async move {
    ///     let mut conn = client
    ///         .get(url)
    ///         .with_request_headers([
    ///             ("some-request-header", "header-value"),
    ///             ("some-other-req-header", "other-header-value"),
    ///         ])
    ///         .await?;
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

    /// ```
    /// let handler = |conn: trillium::Conn| async move {
    ///     conn.with_response_header("some-header", "some-value")
    ///         .with_status(200)
    /// };
    ///
    /// use trillium_client::Client;
    /// use trillium_testing::client_config;
    ///
    /// trillium_testing::with_server(handler, move |url| async move {
    ///     let client = Client::new(client_config());
    ///     let conn = client.get(url).await?;
    ///
    ///     let headers = conn.response_headers(); //<-
    ///
    ///     assert_eq!(headers.get_str("some-header"), Some("some-value"));
    ///     Ok(())
    /// })
    /// ```
    pub fn response_headers(&self) -> &Headers {
        &self.response_headers
    }

    /// retrieves a mutable borrow of the request headers, suitable for
    /// appending a header. generally, prefer using chainable methods on
    /// Conn
    ///
    /// ```
    /// use trillium_client::Client;
    /// use trillium_testing::client_config;
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
    /// let client = Client::new(client_config());
    ///
    /// trillium_testing::with_server(handler, move |url| async move {
    ///     let mut conn = client.get(url);
    ///
    ///     conn.request_headers_mut() //<-
    ///         .insert("some-request-header", "header-value");
    ///
    ///     let mut conn = conn.await?;
    ///
    ///     assert_eq!(
    ///         conn.response_body().read_string().await?,
    ///         "some-request-header was header-value"
    ///     );
    ///     Ok(())
    /// })
    /// ```
    pub fn request_headers_mut(&mut self) -> &mut Headers {
        &mut self.request_headers
    }

    /// get a mutable borrow of the response headers
    pub fn response_headers_mut(&mut self) -> &mut Headers {
        &mut self.response_headers
    }

    /// sets the request body on a mutable reference. prefer the chainable
    /// [`Conn::with_body`] wherever possible
    ///
    /// ```
    /// env_logger::init();
    /// use trillium_client::Client;
    /// use trillium_testing::client_config;
    ///
    /// let handler = |mut conn: trillium::Conn| async move {
    ///     let body = conn.request_body_string().await.unwrap();
    ///     conn.ok(format!("request body was: {}", body))
    /// };
    ///
    /// trillium_testing::with_server(handler, move |url| async move {
    ///     let client = Client::new(client_config());
    ///     let mut conn = client.post(url);
    ///
    ///     conn.set_request_body("body"); //<-
    ///
    ///     (&mut conn).await?;
    ///
    ///     assert_eq!(
    ///         conn.response_body().read_string().await?,
    ///         "request body was: body"
    ///     );
    ///     Ok(())
    /// });
    /// ```
    pub fn set_request_body(&mut self, body: impl Into<Body>) {
        self.request_body = Some(body.into());
    }

    /// chainable setter for the request body
    ///
    /// ```
    /// env_logger::init();
    /// use trillium_client::Client;
    /// use trillium_testing::client_config;
    ///
    /// let handler = |mut conn: trillium::Conn| async move {
    ///     let body = conn.request_body_string().await.unwrap();
    ///     conn.ok(format!("request body was: {}", body))
    /// };
    ///
    /// trillium_testing::with_server(handler, |url| async move {
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
    pub fn with_body(mut self, body: impl Into<Body>) -> Self {
        self.set_request_body(body);
        self
    }

    /// chainable setter for json body. this requires the `json` crate feature to be enabled.
    #[cfg(feature = "json")]
    pub fn with_json_body(self, body: &impl serde::Serialize) -> serde_json::Result<Self> {
        use trillium_http::KnownHeaderName;

        Ok(self
            .with_body(serde_json::to_string(body)?)
            .with_request_header(KnownHeaderName::ContentType, "application/json"))
    }

    pub(crate) fn response_encoding(&self) -> &'static Encoding {
        encoding(&self.response_headers)
    }

    /// retrieves the url for this conn.
    /// ```
    /// use trillium_client::Client;
    /// let client = Client::from(trillium_testing::client_config());
    ///
    /// let conn = client.get("http://localhost:9080");
    ///
    /// let url = conn.url(); //<-
    ///
    /// assert_eq!(url.host_str().unwrap(), "localhost");
    /// ```
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// retrieves the url for this conn.
    /// ```
    /// use trillium_client::Client;
    /// use trillium_testing::{client_config, prelude::*};
    ///
    /// let client = Client::from(client_config());
    /// let conn = client.get("http://localhost:9080");
    ///
    /// let method = conn.method(); //<-
    ///
    /// assert_eq!(method, Method::Get);
    /// ```
    pub fn method(&self) -> Method {
        self.method
    }

    /// returns a [`ReceivedBody`] that borrows the connection inside this conn.
    /// ```
    /// env_logger::init();
    /// use trillium_client::Client;
    /// use trillium_testing::client_config;
    ///
    /// let handler = |mut conn: trillium::Conn| async move { conn.ok("hello from trillium") };
    ///
    /// trillium_testing::with_server(handler, |url| async move {
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
    pub fn response_body(&mut self) -> ReceivedBody<'_, BoxedTransport> {
        ReceivedBody::new(
            self.response_content_length(),
            &mut self.buffer,
            self.transport.as_mut().unwrap(),
            &mut self.response_body_state,
            None,
            encoding(&self.response_headers),
        )
    }

    /// Attempt to deserialize the response body. Note that this consumes the body content.
    #[cfg(feature = "json")]
    pub async fn response_json<T>(&mut self) -> Result<T, ClientSerdeError>
    where
        T: serde::de::DeserializeOwned,
    {
        let body = self.response_body().read_string().await?;
        Ok(serde_json::from_str(&body)?)
    }

    /// returns the status code for this conn. if the conn has not yet
    /// been sent, this will be None.
    ///
    /// ```
    /// use trillium_client::Client;
    /// use trillium_testing::{client_config, prelude::*};
    ///
    /// async fn handler(conn: trillium::Conn) -> trillium::Conn {
    ///     conn.with_status(418)
    /// }
    ///
    /// trillium_testing::with_server(handler, |url| async move {
    ///     let client = Client::new(client_config());
    ///     let conn = client.get(url).await?;
    ///     assert_eq!(Status::ImATeapot, conn.status().unwrap());
    ///     Ok(())
    /// });
    /// ```
    pub fn status(&self) -> Option<Status> {
        self.status
    }

    /// Returns the conn or an [`UnexpectedStatusError`] that contains the conn
    ///
    /// ```
    /// use trillium_testing::client_config;
    ///
    /// trillium_testing::with_server(trillium::Status::NotFound, |url| async move {
    ///     let client = trillium_client::Client::new(client_config());
    ///     assert_eq!(
    ///         client.get(url).await?.success().unwrap_err().to_string(),
    ///         "expected a success (2xx) status code, but got 404 Not Found"
    ///     );
    ///     Ok(())
    /// });
    ///
    /// trillium_testing::with_server(trillium::Status::Ok, |url| async move {
    ///     let client = trillium_client::Client::new(client_config());
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

    /// set the timeout for this conn
    ///
    /// this can also be set on the client with [`Client::set_timeout`] and [`Client::with_timeout`]
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = Some(timeout);
    }

    /// set the timeout for this conn
    ///
    /// this can also be set on the client with [`Client::set_timeout`] and [`Client::with_timeout`]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.set_timeout(timeout);
        self
    }

    /// returns the http version for this conn.
    ///
    /// prior to conn execution, this reflects the request http version that will be sent, and after
    /// execution this reflects the server-indicated http version
    pub fn http_version(&self) -> Version {
        self.http_version
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
