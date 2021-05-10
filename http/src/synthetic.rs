use futures_lite::io::{AsyncRead, AsyncWrite, Result};
use http_types::{
    headers::{Headers, CONTENT_LENGTH},
    Extensions, Method, Version,
};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

use crate::{received_body::ReceivedBodyState, Conn, Stopper};

/**
Synthetic represents a simple transport that contains fixed
content. This is exclusively useful for testing or for server
implementations that are not read from an io connection, such as a
faas function, in which the entire body may be available immediately
on invocation.
*/
#[derive(Debug)]
pub struct Synthetic(Option<Vec<u8>>, usize);
impl AsyncWrite for Synthetic {
    fn poll_write(self: Pin<&mut Self>, _cx: &mut Context<'_>, _buf: &[u8]) -> Poll<Result<usize>> {
        Poll::Ready(Ok(0))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for Synthetic {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        match &self.0 {
            Some(bytes) => {
                let bytes_left = bytes.len() - self.1;
                let bytes_to_read = bytes_left.min(buf.len());
                buf.copy_from_slice(&bytes[self.1..self.1 + bytes_to_read]);
                Poll::Ready(Ok(bytes_to_read))
            }
            None => Poll::Ready(Ok(0)),
        }
    }
}

impl Conn<Synthetic> {
    /**
    Construct a new synthetic conn with provided method, path, and body.
    ```rust
    # use trillium_http::{http_types::Method, Conn};
    let conn = Conn::new_synthetic(Method::Get, "/", Some(b"hello"));
    assert_eq!(conn.method(), &Method::Get);
    assert_eq!(conn.path(), "/");
    ```
    */
    pub fn new_synthetic(method: Method, path: impl Into<String>, body: Option<&[u8]>) -> Self {
        let mut request_headers = Headers::new();
        request_headers.insert(
            CONTENT_LENGTH,
            body.map(|b| b.len()).unwrap_or_default().to_string(),
        );

        Self {
            transport: Synthetic(body.map(|body| body.to_owned()), 0),
            request_headers,
            response_headers: Headers::new(),
            path: path.into(),
            method,
            status: None,
            version: Version::Http1_1,
            state: Extensions::new(),
            response_body: None,
            buffer: None,
            request_body_state: ReceivedBodyState::Start,
            secure: false,
            stopper: Stopper::new(),
        }
    }

    /**
    A Conn<Synthetic> provides the ability to mutate request headers
    with `request_headers_mut`. This is only provided on synthetic
    requests for now, since it doesn't generally make sense to mutate
    headers for a request that is read from an io transport.

    ```rust
    # use trillium_http::{http_types::Method, Conn};
    let mut conn = Conn::new_synthetic(Method::Get, "/", Some(b"hello"));
    conn.request_headers_mut().insert("content-type", "application/json");
    assert_eq!(conn.request_headers()["content-type"], "application/json");
    ```
    */
    pub fn request_headers_mut(&mut self) -> &mut Headers {
        &mut self.request_headers
    }
}
