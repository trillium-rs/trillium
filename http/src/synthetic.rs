use crate::{
    conn::AfterSend, received_body::ReceivedBodyState, Conn, Headers, KnownHeaderName, Method,
    StateSet, Stopper, Version,
};
use futures_lite::io::{AsyncRead, AsyncWrite, Result};
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};

/**
Synthetic represents a simple transport that contains fixed
content. This is exclusively useful for testing or for server
implementations that are not read from an io connection, such as a
faas function, in which the entire body may be available immediately
on invocation.
*/
#[derive(Debug)]
pub struct Synthetic(Option<Vec<u8>>, usize);

impl Synthetic {
    /// the length of this synthetic transport's body
    pub fn len(&self) -> Option<usize> {
        self.0.as_ref().map(Vec::len)
    }

    /// predicate to determine if this synthetic contains no content
    pub fn is_empty(&self) -> bool {
        self.0.as_ref().map(|v| v.is_empty()).unwrap_or(true)
    }
}

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

impl From<Vec<u8>> for Synthetic {
    fn from(v: Vec<u8>) -> Self {
        Some(v).into()
    }
}

impl From<&[u8]> for Synthetic {
    fn from(v: &[u8]) -> Self {
        v.to_owned().into()
    }
}

impl From<String> for Synthetic {
    fn from(v: String) -> Self {
        v.into_bytes().into()
    }
}

impl From<&str> for Synthetic {
    fn from(v: &str) -> Self {
        v.as_bytes().into()
    }
}

impl From<()> for Synthetic {
    fn from(_: ()) -> Self {
        Self(None, 0)
    }
}

impl From<Option<Vec<u8>>> for Synthetic {
    fn from(v: Option<Vec<u8>>) -> Self {
        Self(v, 0)
    }
}

impl Conn<Synthetic> {
    /**
    Construct a new synthetic conn with provided method, path, and body.
    ```rust
    # use trillium_http::{Method, Conn};
    let conn = Conn::new_synthetic(Method::Get, "/", "hello");
    assert_eq!(conn.method(), Method::Get);
    assert_eq!(conn.path(), "/");
    ```
    */
    pub fn new_synthetic(
        method: Method,
        path: impl Into<String>,
        body: impl Into<Synthetic>,
    ) -> Self {
        let transport = body.into();
        let mut request_headers = Headers::new();
        request_headers.insert(
            KnownHeaderName::ContentLength,
            transport.len().unwrap_or_default().to_string(),
        );

        Self {
            transport,
            request_headers,
            response_headers: Headers::new(),
            path: path.into(),
            method,
            status: None,
            version: Version::Http1_1,
            state: StateSet::new(),
            response_body: None,
            buffer: None,
            request_body_state: ReceivedBodyState::Start,
            secure: false,
            stopper: Stopper::new(),
            after_send: AfterSend::default(),
            start_time: Instant::now(),
            peer_ip: None,
        }
    }

    /**
    Replaces the synthetic body. This is intended for testing use.
     */
    pub fn replace_body(&mut self, body: impl Into<Synthetic>) {
        let transport = body.into();
        self.request_headers_mut().insert(
            KnownHeaderName::ContentLength,
            transport.len().unwrap_or_default().to_string(),
        );
        self.transport = transport;
        self.request_body_state = Default::default();
    }
}
