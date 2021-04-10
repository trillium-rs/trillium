use crate::{ClientTransport, Conn, Pool};
use std::convert::TryInto;
use std::fmt::{self, Debug, Formatter};
use trillium::http_types::Method;
use url::Url;

macro_rules! method {
    ($fn_name:ident, $method:ident) => {
        pub fn $fn_name<U>(&self, url: U) -> Conn<'_, Transport>
        where
            <U as TryInto<Url>>::Error: Debug,
            U: TryInto<Url>,
        {
            self.conn(Method::$method, url)
        }
    };
}

impl<Transport: ClientTransport> Client<Transport> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_default_pool(mut self) -> Self {
        self.pool = Some(Pool::default());
        self
    }

    pub fn with_config(mut self, config: Transport::Config) -> Self {
        self.config = config;
        self
    }

    pub fn conn<U>(&self, method: Method, url: U) -> Conn<'_, Transport>
    where
        <U as TryInto<Url>>::Error: Debug,
        U: TryInto<Url>,
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

pub struct Client<Transport: ClientTransport> {
    config: Transport::Config,
    pool: Option<Pool<Transport>>,
}

impl<Transport: ClientTransport> Clone for Client<Transport> {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            pool: self.pool.clone(),
        }
    }
}

impl<Transport: ClientTransport> Default for Client<Transport> {
    fn default() -> Self {
        Self {
            config: Transport::Config::default(),
            pool: None,
        }
    }
}

impl<Transport: ClientTransport> Debug for Client<Transport> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("config", &self.config)
            .field("pool", &self.pool)
            .finish()
    }
}
