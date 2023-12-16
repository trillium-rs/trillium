use crate::{client_handler::ClientHandler, ClientLike, Conn, Pool};
use arc_swap::ArcSwapOption;

use std::{convert::TryInto, fmt::Debug, sync::Arc};
use trillium_http::{transport::BoxedTransport, Method};
use trillium_server_common::{Connector, ObjectSafeConnector, Url};
use url::Origin;

/**
A client contains a Config and an optional connection pool and builds
conns.

*/

#[derive(Clone, Debug)]
pub struct Client(Arc<ClientInner>);

#[derive(Debug)]
pub struct ClientInner {
    config: Box<dyn ObjectSafeConnector>,
    pool: ArcSwapOption<Pool<Origin, BoxedTransport>>,
    handler: ArcSwapOption<Box<dyn ClientHandler>>,
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
        pub fn $fn_name<U>(&self, url: U) -> Conn
        where
            <U as TryInto<Url>>::Error: Debug,
            U: TryInto<Url>,
        {
            self.build_conn(Method::$method, url)
        }
    };
}
impl Client {
    /// builds a new client from this `Connector`
    pub fn new(config: impl Connector) -> Self {
        Self(Arc::new(ClientInner {
            config: config.boxed(),
            pool: ArcSwapOption::empty(),
            handler: ArcSwapOption::empty(),
        }))
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
    pub fn with_default_pool(self) -> Self {
        self.0.pool.store(Some(Arc::new(Pool::default())));
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
    pub fn build_conn<M, U>(&self, method: M, url: U) -> Conn
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: Debug,
        U: TryInto<Url>,
        <U as TryInto<Url>>::Error: Debug,
    {
        let mut conn = Conn::new_with_client(
            self.clone(),
            method.try_into().unwrap(),
            url.try_into().unwrap(),
        );

        if let Some(pool) = self.0.pool.load_full().as_deref() {
            conn.set_pool(pool.clone());
        }

        conn
    }

    /**
    The pool implementation currently accumulates a small memory
    footprint for each new host. If your application is reusing a pool
    against a large number of unique hosts, call this method
    intermittently.
    */
    pub fn clean_up_pool(&self) {
        if let Some(pool) = &*self.0.pool.load() {
            pool.cleanup();
        }
    }

    method!(get, Get);
    method!(post, Post);
    method!(put, Put);
    method!(delete, Delete);
    method!(patch, Patch);

    pub(crate) fn handler(&self) -> Option<Arc<Box<dyn ClientHandler>>> {
        self.0.handler.load_full()
    }
    ///
    pub fn with_handler(mut self, handler: impl ClientHandler) -> Self {
        self.set_handler(handler);
        self
    }
    ///
    pub fn set_handler(&mut self, handler: impl ClientHandler) {
        self.0.handler.store(Some(Arc::new(Box::new(handler))))
    }

    pub(crate) fn connector(&self) -> &dyn ObjectSafeConnector {
        &self.0.config
    }
}

impl<T: Connector> From<T> for Client {
    fn from(connector: T) -> Self {
        Self::new(connector)
    }
}

impl ClientLike for Client {
    fn build_conn(&self, method: Method, url: Url) -> Conn {
        Client::build_conn(self, method, url)
    }
}
