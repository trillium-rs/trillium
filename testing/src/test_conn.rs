use crate::{block_on, AsyncReadExt, Method};
use std::{
    convert::TryInto,
    fmt::Debug,
    ops::{Deref, DerefMut},
};
use trillium::{http_types::headers::Header, Conn, Handler};
use trillium_http::{Conn as HttpConn, Synthetic};

type SyntheticConn = HttpConn<Synthetic>;

#[derive(Debug)]
pub struct TestConn(Conn);

impl TestConn {
    pub fn build<M>(method: M, path: impl Into<String>, body: impl Into<Synthetic>) -> Self
    where
        M: TryInto<Method>,
        <M as TryInto<Method>>::Error: Debug,
    {
        Self(HttpConn::new_synthetic(method.try_into().unwrap(), path.into(), body).into())
    }

    pub fn with_request_header(self, header: impl Header) -> Self {
        let mut inner: SyntheticConn = self.into();
        inner.request_headers_mut().apply(header);
        Self(inner.into())
    }

    pub fn with_request_body(self, body: impl Into<Synthetic>) -> Self {
        let mut inner: SyntheticConn = self.into();
        inner.replace_body(body);
        Self(inner.into())
    }

    pub async fn run_async(self, handler: &impl Handler) -> Self {
        let conn = handler.run(self.into()).await;
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

impl From<TestConn> for Conn {
    fn from(tc: TestConn) -> Self {
        tc.0
    }
}

impl From<TestConn> for SyntheticConn {
    fn from(tc: TestConn) -> Self {
        tc.0.into_inner()
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
