#![forbid(unsafe_code)]
#![warn(
    missing_copy_implementations,
    missing_crate_level_docs,
    missing_debug_implementations,
    missing_docs,
    nonstandard_style,
    unused_qualifications
)]

use async_io::Timer;
use std::{
    convert::TryInto,
    future::Future,
    ops::{Deref, DerefMut},
    time::Duration,
};
use trillium::{Conn, Handler};
use trillium_http::Synthetic;

mod assertions;

mod test_io;
pub use test_io::{CloseableCursor, TestTransport};

// these exports are used by macros
pub use futures_lite::{future::block_on, AsyncRead, AsyncReadExt, AsyncWrite};
pub use trillium_http::http_types::{Method, StatusCode, Url};

#[derive(Debug)]
pub struct TestConn(Conn);

macro_rules! test_conn_method {
    ($fn_name:ident, $method:ident) => {
        pub fn $fn_name(path: impl Into<String>) -> $crate::TestConn {
            $crate::TestConn::build($crate::Method::$method, path, ())
        }
    };
}

pub mod methods {
    test_conn_method!(get, Get);
    test_conn_method!(post, Post);
    test_conn_method!(put, Put);
    test_conn_method!(delete, Delete);
    test_conn_method!(patch, Patch);
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

    pub fn into_inner(self) -> trillium_http::Conn<Synthetic> {
        self.0.into_inner()
    }

    pub fn with_header(self, header: impl trillium::http_types::headers::Header) -> Self {
        let mut inner = self.0.into_inner();
        inner.request_headers_mut().apply(header);
        Self(inner.into())
    }

    pub fn with_request_body(self, body: impl Into<Synthetic>) -> Self {
        let mut inner = self.into_inner();
        inner.replace_body(body);
        Self(inner.into())
    }

    pub async fn run_async(self, handler: &impl Handler) -> Self {
        let conn = handler.run(self.0).await;
        Self(handler.before_send(conn).await)
    }

    pub fn on(self, handler: &impl Handler) -> Self {
        self.run(handler)
    }

    pub fn run(self, handler: &impl Handler) -> Self {
        block_on(self.run_async(handler))
    }

    pub fn take_body_string(&mut self) -> Option<String> {
        self.take_response_body().map(|mut body| {
            let mut s = String::new();
            block_on(body.read_to_string(&mut s)).expect("read");
            s
        })
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

pub fn build_conn<M>(method: M, path: impl Into<String>, body: impl Into<Synthetic>) -> Conn
where
    M: TryInto<Method>,
    <M as TryInto<Method>>::Error: std::fmt::Debug,
{
    trillium_http::Conn::new_synthetic(method.try_into().unwrap(), path, body).into()
}

pub fn serve_once<H, Fun, Fut>(handler: H, tests: Fun)
where
    H: Handler,
    Fun: Fn(Url) -> Fut,
    Fut: Future<Output = Result<(), Box<dyn std::error::Error>>>,
{
    async_global_executor::block_on(async move {
        let port = portpicker::pick_unused_port().expect("could not pick a port");
        let url = format!("http://localhost:{}", port).parse().unwrap();
        let stopper = trillium_smol::Stopper::new();

        let server_future = async_global_executor::spawn(
            trillium_smol::config()
                .with_host("localhost")
                .with_port(port)
                .with_stopper(stopper.clone())
                .run_async(handler),
        );

        Timer::after(Duration::from_millis(500)).await;
        let result = tests(url).await;
        stopper.stop();
        server_future.await;
        result.unwrap()
    })
}
