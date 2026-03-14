use crate::{Conn, IntoUrl, Pool, USER_AGENT, h3::H3ClientState};
use std::{fmt::Debug, sync::Arc, time::Duration};
use trillium_http::{
    HeaderName, HeaderValues, Headers, KnownHeaderName, Method, ReceivedBodyState, ServerConfig,
    TypeSet, Version::Http1_1,
};
use trillium_server_common::{
    ArcedConnector, ArcedQuicClientConfig, Connector, QuicClientConfig, Transport,
    url::{Origin, Url},
};

/// A client contains a Config and an optional connection pool and builds
/// conns.
#[derive(Clone, Debug, fieldwork::Fieldwork)]
pub struct Client {
    config: ArcedConnector,
    h3: Option<H3ClientState>,
    pool: Option<Pool<Origin, Box<dyn Transport>>>,

    /// url base for this client
    #[field(get)]
    base: Option<Arc<Url>>,

    /// default request headers
    #[field(get)]
    default_headers: Arc<Headers>,

    /// optional timeout
    #[field(get, set, with, copy, option_set_some)]
    timeout: Option<Duration>,

    /// configuration
    #[field(get, get_mut, set, with, into)]
    server_config: Arc<ServerConfig>,
}

macro_rules! method {
    ($fn_name:ident, $method:ident) => {
        method!(
            $fn_name,
            $method,
            concat!(
                // yep, macro-generated doctests
                "Builds a new client conn with the ",
                stringify!($fn_name),
                " http method and the provided url.

```
# use trillium_testing::prelude::*;
# use trillium_smol::ClientConfig;
# use trillium_client::Client;
let client = Client::new(ClientConfig::default());
let conn = client.",
                stringify!($fn_name),
                "(\"http://localhost:8080/some/route\"); //<-

assert_eq!(conn.method(), Method::",
                stringify!($method),
                ");
assert_eq!(conn.url().to_string(), \"http://localhost:8080/some/route\");
```
"
            )
        );
    };

    ($fn_name:ident, $method:ident, $doc_comment:expr_2021) => {
        #[doc = $doc_comment]
        pub fn $fn_name(&self, url: impl IntoUrl) -> Conn {
            self.build_conn(Method::$method, url)
        }
    };
}

pub(crate) fn default_request_headers() -> Headers {
    Headers::new()
        .with_inserted_header(KnownHeaderName::UserAgent, USER_AGENT)
        .with_inserted_header(KnownHeaderName::Accept, "*/*")
}

impl Client {
    method!(get, Get);

    method!(post, Post);

    method!(put, Put);

    method!(delete, Delete);

    method!(patch, Patch);

    /// builds a new client from this `Connector`
    pub fn new(connector: impl Connector) -> Self {
        Self {
            config: ArcedConnector::new(connector),
            h3: None,
            pool: None,
            base: None,
            default_headers: Arc::new(default_request_headers()),
            timeout: None,
            server_config: Default::default(),
        }
    }

    /// Build a new client with both a TCP connector and a QUIC connector for HTTP/3 support.
    ///
    /// The connector's runtime and UDP socket type are bound to the QUIC connector here,
    /// before type erasure, so that `trillium-quinn` and the runtime adapter remain
    /// independent crates that neither depends on the other.
    ///
    /// When H3 is configured, the client will track `Alt-Svc` headers in responses and
    /// automatically use HTTP/3 for subsequent requests to origins that advertise it.
    /// Requests to origins without a cached alt-svc entry continue to use HTTP/1.1.
    pub fn new_with_quic<C: Connector, Q: QuicClientConfig<C>>(connector: C, quic: Q) -> Self {
        // Bind the runtime into the QUIC client config before consuming `connector`.
        let arced_quic = ArcedQuicClientConfig::new(&connector, quic);
        Self {
            config: ArcedConnector::new(connector),
            h3: Some(H3ClientState::new(arced_quic)),
            pool: None,
            base: None,
            default_headers: Arc::new(default_request_headers()),
            timeout: None,
            server_config: Default::default(),
        }
    }

    /// chainable method to remove a header from default request headers
    pub fn without_default_header(mut self, name: impl Into<HeaderName<'static>>) -> Self {
        self.default_headers_mut().remove(name);
        self
    }

    /// chainable method to insert a new default request header, replacing any existing value
    pub fn with_default_header(
        mut self,
        name: impl Into<HeaderName<'static>>,
        value: impl Into<HeaderValues>,
    ) -> Self {
        self.default_headers_mut().insert(name, value);
        self
    }

    /// borrow the default headers mutably
    ///
    /// calling this will copy-on-write if the default headers are shared with another client clone
    pub fn default_headers_mut(&mut self) -> &mut Headers {
        Arc::make_mut(&mut self.default_headers)
    }

    /// chainable constructor to enable connection pooling. this can be
    /// combined with [`Client::with_config`]
    ///
    ///
    /// ```
    /// use trillium_client::Client;
    /// use trillium_smol::ClientConfig;
    ///
    /// let client = Client::new(ClientConfig::default()).with_default_pool();
    /// ```
    pub fn with_default_pool(mut self) -> Self {
        self.pool = Some(Pool::default());
        self
    }

    /// builds a new conn.
    ///
    /// if the client has pooling enabled and there is
    /// an available connection to the dns-resolved socket (ip and port),
    /// the new conn will reuse that when it is sent.
    ///
    /// ```
    /// use trillium_client::Client;
    /// use trillium_smol::ClientConfig;
    /// use trillium_testing::prelude::*;
    /// let client = Client::new(ClientConfig::default());
    ///
    /// let conn = client.build_conn("get", "http://trillium.rs"); //<-
    ///
    /// assert_eq!(conn.method(), Method::Get);
    /// assert_eq!(conn.url().host_str().unwrap(), "trillium.rs");
    /// ```
    pub fn build_conn<M>(&self, method: M, url: impl IntoUrl) -> Conn
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: Debug,
    {
        Conn {
            url: self.build_url(url).unwrap(),
            method: method.try_into().unwrap(),
            request_headers: Headers::clone(&self.default_headers),
            response_headers: Headers::new(),
            transport: None,
            status: None,
            request_body: None,
            pool: self.pool.clone(),
            h3: self.h3.clone(),
            buffer: Vec::with_capacity(128).into(),
            response_body_state: ReceivedBodyState::Start,
            config: self.config.clone(),
            headers_finalized: false,
            timeout: self.timeout,
            http_version: Http1_1,
            max_head_length: 8 * 1024,
            state: TypeSet::new(),
            server_config: self.server_config.clone(),
        }
    }

    /// borrow the connector for this client
    pub fn connector(&self) -> &ArcedConnector {
        &self.config
    }

    /// The pool implementation currently accumulates a small memory
    /// footprint for each new host. If your application is reusing a pool
    /// against a large number of unique hosts, call this method
    /// intermittently.
    pub fn clean_up_pool(&self) {
        if let Some(pool) = &self.pool {
            pool.cleanup();
        }
    }

    /// chainable method to set the base for this client
    pub fn with_base(mut self, base: impl IntoUrl) -> Self {
        self.set_base(base).unwrap();
        self
    }

    /// attempt to build a url from this IntoUrl and the [`Client::base`], if set
    pub fn build_url(&self, url: impl IntoUrl) -> crate::Result<Url> {
        url.into_url(self.base())
    }

    /// set the base for this client
    pub fn set_base(&mut self, base: impl IntoUrl) -> crate::Result<()> {
        let mut base = base.into_url(None)?;

        if !base.path().ends_with('/') {
            log::warn!("appending a trailing / to {base}");
            base.set_path(&format!("{}/", base.path()));
        }

        self.base = Some(Arc::new(base));
        Ok(())
    }
}

impl<T: Connector> From<T> for Client {
    fn from(connector: T) -> Self {
        Self::new(connector)
    }
}
