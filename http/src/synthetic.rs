use crate::{
    Conn, Headers, KnownHeaderName, Method, ServerConfig, TypeSet, Version, after_send::AfterSend,
    http_config::DEFAULT_CONFIG, received_body::ReceivedBodyState, transport::Transport,
};
use futures_lite::io::{AsyncRead, AsyncWrite, Cursor, Result};
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

/// Synthetic represents a simple transport that contains fixed
/// content. This is exclusively useful for testing or for server
/// implementations that are not read from an io connection, such as a
/// faas function, in which the entire body may be available immediately
/// on invocation.
#[derive(Debug)]
pub struct Synthetic {
    data: Cursor<Vec<u8>>,
    closed: bool,
}

impl AsyncRead for Synthetic {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        let Synthetic { data, closed } = &mut *self;
        if *closed {
            Poll::Ready(Ok(0))
        } else {
            match Pin::new(data).poll_read(cx, buf) {
                Poll::Ready(Ok(0)) => Poll::Pending,
                other => other,
            }
        }
    }
}

impl Synthetic {
    /// the length of this synthetic transport's body
    pub fn len(&self) -> usize {
        self.data.get_ref().len()
    }

    /// predicate to determine if this synthetic contains no content
    pub fn is_empty(&self) -> bool {
        self.data.get_ref().is_empty()
    }

    /// close this connection
    pub fn close(&mut self) {
        self.closed = true;
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
    fn from(data: Cursor<Vec<u8>>) -> Self {
        Self {
            data,
            closed: false,
        }
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
    /// Construct a new synthetic conn with provided method, path, and body.
    /// ```rust
    /// # use trillium_http::{Method, Conn};
    /// let conn = Conn::new_synthetic(Method::Get, "/", "hello");
    /// assert_eq!(conn.method(), Method::Get);
    /// assert_eq!(conn.path(), "/");
    /// ```
    pub fn new_synthetic(
        method: Method,
        path: impl Into<String>,
        body: impl Into<Synthetic>,
    ) -> Self {
        let transport = body.into();
        let mut request_headers = Headers::new();
        request_headers.insert(KnownHeaderName::ContentLength, transport.len().to_string());

        Self {
            server_config: Arc::default(),
            transport,
            request_headers,
            response_headers: Headers::new(),
            path: path.into(),
            method,
            status: None,
            version: Version::Http1_1,
            state: TypeSet::new(),
            response_body: None,
            buffer: Vec::with_capacity(DEFAULT_CONFIG.request_buffer_initial_len).into(),
            request_body_state: ReceivedBodyState::Start,
            secure: false,
            after_send: AfterSend::default(),
            start_time: Instant::now(),
            peer_ip: None,
        }
    }

    /// use a particular shared server config for this synthetic conn
    pub fn set_server_config(&mut self, server_config: Arc<ServerConfig>) {
        self.server_config = server_config;
    }

    /// chainable setter for server config
    #[must_use]
    pub fn with_server_config(mut self, server_config: Arc<ServerConfig>) -> Self {
        self.set_server_config(server_config);
        self
    }

    /// simulate closing the transport
    pub fn close(&mut self) {
        self.transport.close();
    }

    /// Replaces the synthetic body. This is intended for testing use.
    pub fn replace_body(&mut self, body: impl Into<Synthetic>) {
        let transport = body.into();
        self.request_headers_mut()
            .insert(KnownHeaderName::ContentLength, transport.len().to_string());
        self.transport = transport;
        self.request_body_state = ReceivedBodyState::default();
    }
}
