use crate::{Conn, Pool};
use std::convert::TryInto;
use std::fmt::{self, Debug, Formatter};
use trillium::http_types::Method;
use trillium_tls_common::{Connector, Url};

macro_rules! method {
    ($fn_name:ident, $method:ident) => {
        pub fn $fn_name<U>(&self, url: U) -> Conn<'_, C>
        where
            <U as TryInto<Url>>::Error: Debug,
            U: TryInto<Url>,
        {
            self.conn(Method::$method, url)
        }
    };
}

impl<C: Connector> Client<C> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_default_pool(mut self) -> Self {
        self.pool = Some(Pool::default());
        self
    }

    pub fn with_config(mut self, config: C::Config) -> Self {
        self.config = config;
        self
    }

    pub fn conn<U>(&self, method: Method, url: U) -> Conn<'_, C>
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

pub struct Client<C: Connector> {
    config: C::Config,
    pool: Option<Pool<C::Transport>>,
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
