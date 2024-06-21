use crate::{Conn, IntoUrl, Pool, USER_AGENT};
use std::{fmt::Debug, sync::Arc, time::Duration};
use trillium_http::{
    transport::BoxedTransport, HeaderName, HeaderValues, Headers, KnownHeaderName, Method,
    ReceivedBodyState, Version::Http1_1,
};
use trillium_server_common::{
    url::{Origin, Url},
    ArcedConnector, Connector,
};

/**
A client contains a Config and an optional connection pool and builds
conns.

*/
#[derive(Clone, Debug)]
pub struct Client {
    config: ArcedConnector,
    pool: Option<Pool<Origin, BoxedTransport>>,
    base: Option<Arc<Url>>,
    default_headers: Arc<Headers>,
    timeout: Option<Duration>,
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

    ($fn_name:ident, $method:ident, $doc_comment:expr) => {
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
    pub fn new(config: impl Connector) -> Self {
        Self {
            config: ArcedConnector::new(config),
            pool: None,
            base: None,
            default_headers: Arc::new(default_request_headers()),
            timeout: None,
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

    /// borrow the default headers
    pub fn default_headers(&self) -> &Headers {
        &self.default_headers
    }

    /// borrow the default headers mutably
    ///
    /// calling this will copy-on-write if the default headers are shared with another client clone
    pub fn default_headers_mut(&mut self) -> &mut Headers {
        Arc::make_mut(&mut self.default_headers)
    }

    /**
    chainable constructor to enable connection pooling. this can be
    combined with [`Client::with_config`]


    ```
    use trillium_smol::ClientConfig;
    use trillium_client::Client;

    let client = Client::new(ClientConfig::default())
        .with_default_pool(); //<-
    ```
    */
    pub fn with_default_pool(mut self) -> Self {
        self.pool = Some(Pool::default());
        self
    }

    /**
    builds a new conn.

    if the client has pooling enabled and there is
    an available connection to the dns-resolved socket (ip and port),
    the new conn will reuse that when it is sent.

    ```
    use trillium_smol::ClientConfig;
    use trillium_client::Client;
    use trillium_testing::prelude::*;
    let client = Client::new(ClientConfig::default());

    let conn = client.build_conn("get", "http://trillium.rs"); //<-

    assert_eq!(conn.method(), Method::Get);
    assert_eq!(conn.url().host_str().unwrap(), "trillium.rs");
    ```
    */
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
            buffer: Vec::with_capacity(128).into(),
            response_body_state: ReceivedBodyState::Start,
            config: self.config.clone(),
            headers_finalized: false,
            timeout: self.timeout,
            http_version: Http1_1,
            max_head_length: 8 * 1024,
        }
    }

    /// borrow the connector for this client
    pub fn connector(&self) -> &ArcedConnector {
        &self.config
    }

    /**
    The pool implementation currently accumulates a small memory
    footprint for each new host. If your application is reusing a pool
    against a large number of unique hosts, call this method
    intermittently.
    */
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

    /// retrieve the base for this client, if any
    pub fn base(&self) -> Option<&Url> {
        self.base.as_deref()
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

    /// set the timeout for all conns this client builds
    ///
    /// this can also be set with [`Conn::set_timeout`] and [`Conn::with_timeout`]
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = Some(timeout);
    }

    /// set the timeout for all conns this client builds
    ///
    /// this can also be set with [`Conn::set_timeout`] and [`Conn::with_timeout`]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.set_timeout(timeout);
        self
    }
}

impl<T: Connector> From<T> for Client {
    fn from(connector: T) -> Self {
        Self::new(connector)
    }
}
