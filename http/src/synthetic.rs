use crate::{
    after_send::AfterSend, http_config::DEFAULT_CONFIG, received_body::ReceivedBodyState,
    transport::Transport, Conn, Headers, KnownHeaderName, Method, StateSet, Stopper, Version,
};
use futures_lite::io::{AsyncRead, AsyncWrite, Cursor, Result};
use trillium_macros::AsyncRead;
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
#[derive(Debug, AsyncRead)]
pub struct Synthetic(Cursor<Vec<u8>>);

impl Synthetic {
    /// the length of this synthetic transport's body
    pub fn len(&self) -> Option<usize> {
        // this is as such for semver-compatibility with a previous interface
        match self.0.get_ref().len() {
            0 => None,
            n => Some(n)
        }
    }

    /// predicate to determine if this synthetic contains no content
    pub fn is_empty(&self) -> bool {
        self.0.get_ref().is_empty()
    }
}

impl Transport for Synthetic {}

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

impl From<Cursor<Vec<u8>>> for Synthetic {
    fn from(value: Cursor<Vec<u8>>) -> Self {
        Self(value)
    }
}

impl From<Vec<u8>> for Synthetic {
    fn from(v: Vec<u8>) -> Self {
        Cursor::new(v).into()
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
    fn from((): ()) -> Self {
        Vec::new().into()
    }
}

impl From<Option<Vec<u8>>> for Synthetic {
    fn from(v: Option<Vec<u8>>) -> Self {
        v.unwrap_or_default().into()
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
            buffer: Vec::with_capacity(DEFAULT_CONFIG.request_buffer_initial_len).into(),
            request_body_state: ReceivedBodyState::Start,
            secure: false,
            stopper: Stopper::new(),
            after_send: AfterSend::default(),
            start_time: Instant::now(),
            peer_ip: None,
            http_config: DEFAULT_CONFIG,
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
        self.request_body_state = ReceivedBodyState::default();
    }
}
