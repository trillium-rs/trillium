use crate::{ClientTransport, Conn, Pool};
use myco::http_types::Method;

use std::convert::TryInto;
use std::fmt::Debug;
use url::Url;

pub struct Client<T: ClientTransport> {
    config: T::Config,
    pool: Option<Pool<T>>,
}

impl<T: ClientTransport> Clone for Client<T> {
    fn clone(&self) -> Self {
        Self {
            config: self.config.clone(),
            pool: self.pool.clone(),
        }
    }
}

impl<T: ClientTransport> Default for Client<T> {
    fn default() -> Self {
        Self {
            config: T::Config::default(),
            pool: None,
        }
    }
}

impl<T: ClientTransport> std::fmt::Debug for Client<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("config", &self.config)
            .field("pool", &self.pool)
            .finish()
    }
}

macro_rules! method {
    ($fn_name:ident, $method:ident) => {
        pub fn $fn_name<U>(&self, url: U) -> Conn<'_, T>
        where
            <U as TryInto<Url>>::Error: Debug,
            U: TryInto<Url>,
        {
            self.conn(Method::$method, url)
        }
    };
}

impl<T: ClientTransport> Client<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_default_pool(mut self) -> Self {
        self.pool = Some(Pool::default());
        self
    }

    pub fn conn<'a, U>(&'a self, method: Method, url: U) -> Conn<'a, T>
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
