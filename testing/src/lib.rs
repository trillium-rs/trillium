pub use futures_lite;
use myco::Handler;
pub use myco_http::http_types::Method;
use myco_http::Synthetic;
use std::convert::TryInto;

mod assertions;

mod test_io;
pub use test_io::{CloseableCursor, TestIO};

pub mod server;

pub fn test_conn<T>(method: T, path: impl Into<String>, body: Option<Vec<u8>>) -> myco::Conn
where
    T: TryInto<Method>,
    <T as TryInto<Method>>::Error: std::fmt::Debug,
{
    myco::Conn::new(myco_http::Conn::new_synthetic(
        method.try_into().unwrap(),
        path.into(),
        body,
    ))
}

pub fn run(handler: &impl myco::Handler, conn: myco::Conn) -> myco::Conn {
    futures_lite::future::block_on(async move {
        let conn = handler.run(conn).await;
        handler.before_send(conn).await
    })
}

pub struct TestConn(myco_http::Conn<Synthetic>);

macro_rules! test_conn_method {
    ($fn_name:ident, $method:ident) => {
        pub fn $fn_name(path: impl Into<String>) -> Self {
            Self::build(Method::$method, path)
        }
    };
}

impl TestConn {
    pub fn build<M>(method: M, path: impl Into<String>) -> Self
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: std::fmt::Debug,
    {
        Self(myco_http::Conn::new_synthetic(
            method.try_into().unwrap(),
            path.into(),
            None,
        ))
    }

    test_conn_method!(get, Get);
    test_conn_method!(post, Post);
    test_conn_method!(put, Put);
    test_conn_method!(delete, Delete);
    test_conn_method!(patch, Patch);

    pub fn into_inner(self) -> myco_http::Conn<Synthetic> {
        self.0
    }

    pub fn inner_mut(&mut self) -> &mut myco_http::Conn<Synthetic> {
        &mut self.0
    }

    pub fn inner(&self) -> &myco_http::Conn<Synthetic> {
        &self.0
    }

    pub fn run(self, handler: &impl myco::Handler) -> Self {
        let conn = myco::Conn::new(self.0);
        let conn = futures_lite::future::block_on(async move {
            let conn = handler.run(conn).await;
            handler.before_send(conn).await
        });
        Self(conn.into_inner())
    }
}

pub struct TestHandler<H>(H);

macro_rules! test_handler_method {
    ($fn_name:ident, $method:ident) => {
        pub fn $fn_name(&self, path: impl Into<String>) -> TestConn {
            self.request(Method::$method, path)
        }
    };
}

impl<H: Handler> TestHandler<H> {
    pub fn new(handler: H) -> Self {
        Self(handler)
    }

    pub fn request<M>(&self, method: M, path: impl Into<String>) -> TestConn
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: std::fmt::Debug,
    {
        TestConn::build(method, path).run(&self.0)
    }

    test_handler_method!(get, Get);
    test_handler_method!(post, Post);
    test_handler_method!(put, Put);
    test_handler_method!(delete, Delete);
    test_handler_method!(patch, Patch);
}
