use crate::Error;
use encoding_rs::Encoding;
use futures_lite::{AsyncRead, AsyncReadExt};
use std::{
    fmt::{self, Debug, Formatter},
    io,
    pin::Pin,
    task::{Context, Poll},
};
use trillium_http::ReceivedBody;
use trillium_server_common::Transport;

/// A response body received from a server.
///
/// Most of the time this represents a body that will be read from the underlying transport, but it
/// can also wrap a synthetic body installed by middleware via [`Conn::set_response_body`] —
/// e.g. cache hits, mocked responses, or circuit-breaker short-circuits. Reads, encoding handling,
/// and `max_len` enforcement work transparently across both cases.
///
/// [`Conn::set_response_body`]: crate::Conn::set_response_body
///
/// ```rust
/// use trillium_client::Client;
/// use trillium_testing::{client_config, with_server};
///
/// with_server("hello from trillium", |url| async move {
///     let client = Client::new(client_config());
///     let mut conn = client.get(url).await?;
///     let body = conn.response_body(); //<-
///     assert_eq!(Some(19), body.content_length());
///     assert_eq!("hello from trillium", body.read_string().await?);
///     Ok(())
/// });
/// ```
///
/// ## Bounds checking
///
/// Every `ResponseBody` has a maximum length beyond which it will return an error, expressed as a
/// u64. To override this on the specific `ResponseBody`, use [`ResponseBody::with_max_len`] or
/// [`ResponseBody::set_max_len`]. The bound is enforced on synthetic bodies as well as
/// transport-backed ones, so a user-set memory cap holds even when middleware has replaced the
/// body with externally-sourced bytes.
pub struct ResponseBody<'a>(ResponseBodyInner<'a>);

#[allow(clippy::large_enum_variant)]
enum ResponseBodyInner<'a> {
    Received(ReceivedBody<'a, Box<dyn Transport>>),
    Synthetic(SyntheticResponseBody<'a>),
}

pub(crate) struct SyntheticResponseBody<'a> {
    reader: Pin<&'a mut (dyn AsyncRead + Send + Sync)>,
    content_length: Option<u64>,
    encoding: &'static Encoding,
    max_len: u64,
    bytes_read: u64,
}

impl<'a> SyntheticResponseBody<'a> {
    pub(crate) fn new(
        reader: Pin<&'a mut (dyn AsyncRead + Send + Sync)>,
        content_length: Option<u64>,
        encoding: &'static Encoding,
        max_len: u64,
    ) -> Self {
        Self {
            reader,
            content_length,
            encoding,
            max_len,
            bytes_read: 0,
        }
    }
}

impl Debug for ResponseBody<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.0 {
            ResponseBodyInner::Received(rb) => f.debug_tuple("ResponseBody").field(rb).finish(),
            ResponseBodyInner::Synthetic(s) => f
                .debug_struct("ResponseBody (synthetic)")
                .field("content_length", &s.content_length)
                .field("encoding", &s.encoding.name())
                .field("max_len", &s.max_len)
                .field("bytes_read", &s.bytes_read)
                .finish(),
        }
    }
}

impl AsyncRead for ResponseBody<'_> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let inner = &mut self.get_mut().0;
        match inner {
            ResponseBodyInner::Received(rb) => Pin::new(rb).poll_read(cx, buf),
            ResponseBodyInner::Synthetic(s) => {
                let remaining = s.max_len.saturating_sub(s.bytes_read);
                if remaining == 0 && !buf.is_empty() {
                    return Poll::Ready(Err(io::Error::other(Error::ReceivedBodyTooLong(
                        s.max_len,
                    ))));
                }
                let cap = remaining.min(buf.len() as u64) as usize;
                match s.reader.as_mut().poll_read(cx, &mut buf[..cap]) {
                    Poll::Ready(Ok(n)) => {
                        s.bytes_read += n as u64;
                        Poll::Ready(Ok(n))
                    }
                    other => other,
                }
            }
        }
    }
}

impl ResponseBody<'_> {
    /// Similar to [`ResponseBody::read_string`], but returns the raw bytes. This is useful for
    /// bodies that are not text.
    ///
    /// You can use this in conjunction with `encoding` if you need different handling of malformed
    /// character encoding than the lossy conversion provided by [`ResponseBody::read_string`].
    ///
    /// An empty or nonexistent body will yield an empty Vec, not an error.
    ///
    /// # Errors
    ///
    /// This will return an error if there is an IO error on the underlying transport such as a
    /// disconnect.
    ///
    /// This will also return an error if the length exceeds the maximum length. To configure the
    /// value on this specific request body, use [`ResponseBody::with_max_len`] or
    /// [`ResponseBody::set_max_len`]
    pub async fn read_bytes(self) -> Result<Vec<u8>, Error> {
        match self.0 {
            ResponseBodyInner::Received(rb) => rb.read_bytes().await,
            ResponseBodyInner::Synthetic(_) => {
                let mut bytes = Vec::new();
                let mut this = self;
                AsyncReadExt::read_to_end(&mut this, &mut bytes)
                    .await
                    .map_err(downcast_io_error)?;
                Ok(bytes)
            }
        }
    }

    /// # Reads the entire body to `String`.
    ///
    /// This uses the encoding determined by the content-type (mime) charset. If an encoding problem
    /// is encountered, the String returned by [`ResponseBody::read_string`] will contain utf8
    /// replacement characters.
    ///
    /// Note that this can only be performed once per Conn, as the underlying data is not cached
    /// anywhere. This is the only copy of the body contents.
    ///
    /// An empty or nonexistent body will yield an empty String, not an error
    ///
    /// # Errors
    ///
    /// This will return an error if there is an IO error on the
    /// underlying transport such as a disconnect
    ///
    ///
    /// This will also return an error if the length exceeds the maximum length. To configure the
    /// value on this specific response body, use [`ResponseBody::with_max_len`] or
    /// [`ResponseBody::set_max_len`].
    pub async fn read_string(self) -> Result<String, Error> {
        match self.0 {
            ResponseBodyInner::Received(rb) => rb.read_string().await,
            ResponseBodyInner::Synthetic(ref s) => {
                let encoding = s.encoding;
                let bytes = self.read_bytes().await?;
                let (decoded, _, _) = encoding.decode(&bytes);
                Ok(decoded.into_owned())
            }
        }
    }

    /// Set the maximum content length to read, returning self
    ///
    /// This protects against an memory-use denial-of-service attack wherein an untrusted peer sends
    /// an unbounded request body. This is especially important when using
    /// [`ResponseBody::read_string`] and [`ResponseBody::read_bytes`] instead of streaming with
    /// `AsyncRead`.
    ///
    /// The default value can be found documented [in the trillium-http
    /// crate](https://docs.trillium.rs/trillium_http/struct.httpconfig#received_body_max_len)
    #[must_use]
    pub fn with_max_len(mut self, max_len: u64) -> Self {
        self.set_max_len(max_len);
        self
    }

    /// Set the maximum content length to read
    ///
    /// This protects against an memory-use denial-of-service attack wherein an untrusted peer sends
    /// an unbounded request body. This is especially important when using
    /// [`ResponseBody::read_string`] and [`ResponseBody::read_bytes`] instead of streaming with
    /// `AsyncRead`.
    ///
    /// The default value can be found documented [in the trillium-http
    /// crate](https://docs.trillium.rs/trillium_http/struct.httpconfig#received_body_max_len)
    pub fn set_max_len(&mut self, max_len: u64) -> &mut Self {
        match &mut self.0 {
            ResponseBodyInner::Received(rb) => {
                rb.set_max_len(max_len);
            }
            ResponseBodyInner::Synthetic(s) => {
                s.max_len = max_len;
            }
        }
        self
    }

    /// The content-length of this body, if available.
    ///
    /// This value usually is derived from the content-length header. If the request that this body
    /// is attached to uses transfer-encoding chunked, this will be None.
    pub fn content_length(&self) -> Option<u64> {
        match &self.0 {
            ResponseBodyInner::Received(rb) => rb.content_length(),
            ResponseBodyInner::Synthetic(s) => s.content_length,
        }
    }

    pub(crate) async fn drain(self) -> io::Result<u64> {
        match self.0 {
            ResponseBodyInner::Received(rb) => rb.drain().await,
            ResponseBodyInner::Synthetic(_) => {
                let mut this = self;
                let mut buf = [0u8; 4096];
                let mut total = 0u64;
                loop {
                    match AsyncReadExt::read(&mut this, &mut buf).await? {
                        0 => return Ok(total),
                        n => total += n as u64,
                    }
                }
            }
        }
    }
}

impl<'a> From<ReceivedBody<'a, Box<dyn Transport>>> for ResponseBody<'a> {
    fn from(received_body: ReceivedBody<'a, Box<dyn Transport>>) -> Self {
        Self(ResponseBodyInner::Received(received_body))
    }
}

impl<'a> From<SyntheticResponseBody<'a>> for ResponseBody<'a> {
    fn from(synthetic: SyntheticResponseBody<'a>) -> Self {
        Self(ResponseBodyInner::Synthetic(synthetic))
    }
}

impl<'a> IntoFuture for ResponseBody<'a> {
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;
    type Output = trillium_http::Result<String>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.read_string().await })
    }
}

fn downcast_io_error(e: io::Error) -> Error {
    e.downcast::<Error>()
        .unwrap_or_else(|e| Error::Io(io::Error::other(e)))
}
