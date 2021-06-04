use crate::{Conn, Pool};
use std::{
    convert::TryInto,
    fmt::{self, Debug, Formatter},
};
use trillium::http_types::Method;
use trillium_tls_common::{Connector, Url};

/**
A client contains a Config and an optional connection pool and builds
conns.

*/
pub struct Client<C: Connector> {
    config: C::Config,
    pool: Option<Pool<C::Transport>>,
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
# use trillium_smol::TcpConnector;
# use trillium_client::Client;
let client = Client::<TcpConnector>::new();
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
        pub fn $fn_name<U>(&self, url: U) -> Conn<'_, C>
        where
            <U as TryInto<Url>>::Error: Debug,
            U: TryInto<Url>,
        {
            self.build_conn(Method::$method, url)
        }
    };
}

impl<C: Connector> Client<C> {
    /**
    Constructs a new Client without a connection pool and with default
    configuration.
    */
    pub fn new() -> Self {
        Self::default()
    }

    /**
    chainable constructor to enable connection pooling. this can be
    combined with [`Client::with_config`]


    ```
    use trillium_smol::TcpConnector;
    use trillium_client::Client;

    let client = Client::<TcpConnector>::new()
        .with_default_pool(); //<-
    ```
    */
    pub fn with_default_pool(mut self) -> Self {
        self.pool = Some(Pool::default());
        self
    }

    /**
    chainable constructor to specify Connector configuration.  this
    can be combined with [`Client::with_default_pool`]

    ```
    use trillium_smol::{TcpConnector, ClientConfig};
    use trillium_client::Client;

    let client = Client::<TcpConnector>::new()
        .with_config(ClientConfig { //<-
            nodelay: Some(true),
            ..Default::default()
        });
    ```
    */
    pub fn with_config(mut self, config: C::Config) -> Self {
        self.config = config;
        self
    }

    /**
    builds a new conn borrowing the config on this client. if the
    client has pooling enabled and there is an available connection to
    the dns-resolved socket (ip and port), the new conn will reuse
    that when it is sent.

    ```
    use trillium_smol::{TcpConnector, ClientConfig};
    use trillium_client::Client;
    let client = Client::<TcpConnector>::new();

    let conn = client.build_conn("get", "http://trillium.rs"); //<-

    assert_eq!(conn.method(), trillium_testing::Method::Get);
    assert_eq!(conn.url().host_str().unwrap(), "trillium.rs");
    ```
    */
    pub fn build_conn<M, U>(&self, method: M, url: U) -> Conn<'_, C>
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: Debug,
        U: TryInto<Url>,
        <U as TryInto<Url>>::Error: Debug,
    {
        let mut conn = Conn::new(method, url).with_config(&self.config);
        if let Some(pool) = &self.pool {
            conn.set_pool(pool.clone());
        }
        conn
    }

    method!(get, Get);
    method!(post, Post);
    method!(put, Put);
    method!(delete, Delete);
    method!(patch, Patch);
}

impl<C: Connector> Clone for Client<C> {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            pool: self.pool.clone(),
        }
    }
}

impl<C: Connector> Default for Client<C> {
    fn default() -> Self {
        Self {
            config: C::Config::default(),
            pool: None,
        }
    }
}

impl<Transport: Connector> Debug for Client<Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("config", &self.config)
            .field("pool", &self.pool)
            .finish()
    }
}
