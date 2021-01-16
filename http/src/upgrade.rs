use crate::Conn;
use http_types::{Extensions, Headers, Method};
use std::fmt::{self, Debug, Formatter};

pub struct Upgrade<RW> {
    pub request_headers: Headers,
    pub path: String,
    pub method: Method,
    pub state: Extensions,
    pub rw: RW,
    pub buffer: Option<Vec<u8>>,
}

impl<RW> Upgrade<RW> {
    pub fn headers(&self) -> &Headers {
        &self.request_headers
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn method(&self) -> &Method {
        &self.method
    }
    pub fn state(&self) -> &Extensions {
        &self.state
    }
}

impl<RW> Debug for Upgrade<RW> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(&format!("Upgrade<{}>", std::any::type_name::<RW>()))
            .field("request_headers", &self.request_headers)
            .field("path", &self.path)
            .field("method", &self.method)
            .field("buffer", &self.buffer.as_deref().map(utf8))
            .finish()
    }
}

impl<RW> From<Conn<RW>> for Upgrade<RW> {
    fn from(conn: Conn<RW>) -> Self {
        let Conn {
            request_headers,
            path,
            method,
            state,
            rw,
            buffer,
            ..
        } = conn;

        Self {
            request_headers,
            path,
            method,
            state,
            rw,
            buffer,
        }
    }
}
pub fn utf8(d: &[u8]) -> &str {
    std::str::from_utf8(d).unwrap_or("not utf8")
}
