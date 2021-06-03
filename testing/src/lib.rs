#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

pub use futures_lite;
use futures_lite::future;
use std::{
    convert::TryInto,
    ops::{Deref, DerefMut},
};
use trillium::{Conn, Handler};
pub use trillium_http::http_types::Method;
use trillium_http::Synthetic;

mod assertions;

mod test_io;
pub use test_io::{CloseableCursor, TestTransport};

pub mod server;

pub fn test_conn<T>(method: T, path: impl Into<String>, body: impl Into<Synthetic>) -> Conn
where
    T: TryInto<Method>,
    <T as TryInto<Method>>::Error: std::fmt::Debug,
{
    trillium_http::Conn::new_synthetic(method.try_into().unwrap(), path.into(), body).into()
}

pub fn run(handler: &impl Handler, conn: Conn) -> Conn {
    future::block_on(async move {
        let conn = handler.run(conn).await;
        handler.before_send(conn).await
    })
}

#[derive(Debug)]
pub struct TestConn(Conn);

macro_rules! test_conn_method {
    ($fn_name:ident, $method:ident) => {
        pub fn $fn_name(path: impl Into<String>) -> Self {
            Self::build(Method::$method, path, ())
        }
    };
}

impl TestConn {
    pub fn build<M>(method: M, path: impl Into<String>, body: impl Into<Synthetic>) -> Self
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: std::fmt::Debug,
    {
        Self(
            trillium_http::Conn::new_synthetic(method.try_into().unwrap(), path.into(), body)
                .into(),
        )
    }

    test_conn_method!(get, Get);
    test_conn_method!(post, Post);
    test_conn_method!(put, Put);
    test_conn_method!(delete, Delete);
    test_conn_method!(patch, Patch);

    pub fn into_inner(self) -> trillium_http::Conn<Synthetic> {
        self.0.into_inner()
    }

    pub fn with_header(self, header: impl trillium::http_types::headers::Header) -> Self {
        let mut inner = self.0.into_inner();
        inner.request_headers_mut().apply(header);
        Self(inner.into())
    }

    pub async fn run_async(self, handler: &impl Handler) -> Self {
        let conn = handler.run(self.0.into()).await;
        Self(handler.before_send(conn).await)
    }

    pub fn run(self, handler: &impl Handler) -> Self {
        future::block_on(self.run_async(handler))
    }
}

impl Deref for TestConn {
    type Target = Conn;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TestConn {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

macro_rules! test_handler_method {
    ($fn_name:ident, $method:ident) => {
        fn $fn_name(&self, path: impl Into<String>) -> TestConn {
            self.request(Method::$method, path)
        }
    };
}

pub mod methods {
    use super::{Handler, Method, TestConn};
    macro_rules! method {
        ($fn_name:ident, $method:ident) => {
            pub fn $fn_name(handler: &impl Handler, path: impl Into<String>) -> TestConn {
                TestConn::build(Method::$method, path, ()).run(handler)
            }
        };
    }
    method!(get, Get);
    method!(post, Post);
    method!(put, Put);
    method!(delete, Delete);
    method!(patch, Patch);
}

pub trait HandlerTesting {
    fn request<M>(&self, method: M, path: impl Into<String>) -> TestConn
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: std::fmt::Debug;

    test_handler_method!(get, Get);
    test_handler_method!(post, Post);
    test_handler_method!(put, Put);
    test_handler_method!(delete, Delete);
    test_handler_method!(patch, Patch);
}

impl<H> HandlerTesting for H
where
    H: Handler,
{
    fn request<M>(&self, method: M, path: impl Into<String>) -> TestConn
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: std::fmt::Debug,
    {
        TestConn::build(method, path, ()).run(self)
    }
}

pub fn build_conn<M>(method: M, path: impl Into<String>, body: impl Into<Synthetic>) -> Conn
where
    M: TryInto<Method>,
    <M as TryInto<Method>>::Error: std::fmt::Debug,
{
    trillium_http::Conn::new_synthetic(method.try_into().unwrap(), path, body).into()
}
