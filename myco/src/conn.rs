use http_types::{
    headers::{HeaderName, ToHeaderValues},
    Body, Headers, Method, StatusCode, Url,
};
use myco_http::RequestBody;
use std::convert::TryInto;
use std::fmt::{self, Debug, Formatter};

use crate::{BoxedTransport, Grain, Transport};

pub struct Conn {
    inner: myco_http::Conn<BoxedTransport>,
    halted: bool,
    before_send: Option<Vec<Box<dyn Grain>>>,
}

impl Debug for Conn {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("Conn")
            .field("inner", &self.inner)
            .field("halted", &self.halted)
            .field("before_send", &self.before_send.as_ref().map(|b| b.name()))
            .finish()
    }
}

impl Conn {
    pub fn new(conn: myco_http::Conn<impl Transport + 'static>) -> Self {
        Self {
            inner: conn.map_transport(BoxedTransport::new),
            halted: false,
            before_send: None,
        }
    }

    pub fn state<T: 'static>(&self) -> Option<&T> {
        self.inner.state().get()
    }

    pub fn state_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.inner.state_mut().get_mut()
    }

    pub fn register_before_send<G: Grain>(mut self, grain: G) -> Self {
        self.before_send
            .get_or_insert_with(|| vec![])
            .push(Box::new(grain));

        self
    }

    pub async fn request_body<'a>(&'a mut self) -> RequestBody<'a, BoxedTransport> {
        self.inner.request_body().await
    }

    pub fn header_eq_ignore_case(&self, name: HeaderName, value: &str) -> bool {
        match self.headers().get(name) {
            Some(header) => header.as_str().eq_ignore_ascii_case(value),
            None => false,
        }
    }

    pub fn response_len(&self) -> Option<usize> {
        self.inner.response_body().and_then(|b| b.len())
    }

    pub fn method(&self) -> &Method {
        self.inner.method()
    }

    pub fn get_status(&self) -> Option<&StatusCode> {
        self.inner.status()
    }

    pub fn headers(&self) -> &Headers {
        self.inner.request_headers()
    }

    pub fn headers_mut(&mut self) -> &mut Headers {
        self.inner.response_headers()
    }

    pub fn send_header(
        mut self,
        name: impl Into<http_types::headers::HeaderName>,
        values: impl ToHeaderValues,
    ) -> Self {
        self.headers_mut().insert(name, values);
        self
    }

    pub fn append_header(
        mut self,
        name: impl Into<http_types::headers::HeaderName>,
        values: impl ToHeaderValues,
    ) -> Self {
        self.headers_mut().append(name, values);
        self
    }

    pub fn set_state<T: Send + Sync + 'static>(&mut self, val: T) -> Option<T> {
        self.inner.state_mut().insert(val)
    }

    pub fn with_state<T: Send + Sync + 'static>(mut self, val: T) -> Self {
        self.set_state(val);
        self
    }

    pub fn take_state<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.inner.state_mut().remove()
    }

    pub fn status(mut self, status: impl TryInto<http_types::StatusCode>) -> Self {
        self.inner.set_status(status);
        self
    }

    pub fn url(&self) -> Option<Url> {
        self.inner.url().ok()
    }

    pub fn path(&self) -> &str {
        self.inner.path()
    }

    pub fn halt(mut self) -> Self {
        self.halted = true;
        self
    }

    pub fn is_halted(&self) -> bool {
        self.halted
    }

    pub fn body(mut self, body: impl Into<Body>) -> Self {
        self.inner.set_body(body);
        self
    }

    pub fn ok(self, body: impl Into<Body>) -> Conn {
        self.status(200).body(body)
    }

    pub fn secure(&self) -> bool {
        match self.url() {
            Some(url) => url.scheme() == "https",
            None => false,
        }
    }

    pub fn inner(&self) -> &myco_http::Conn<BoxedTransport> {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut myco_http::Conn<BoxedTransport> {
        &mut self.inner
    }

    pub async fn send(mut self) -> Self {
        if let Some(before_send) = self.before_send.take() {
            before_send.run(self).await
        } else {
            self
        }
    }

    pub fn into_inner<T: Transport>(self) -> myco_http::Conn<T> {
        self.inner.map_transport(|t| {
            *t.downcast()
                .expect("attempted to downcast to the wrong transport type")
        })
    }
}
