use crate::{Error, Pool, pool::PoolEntry};
use encoding_rs::Encoding;
use futures_lite::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use std::{
    fmt::{self, Debug, Formatter},
    io, mem,
    pin::Pin,
    task::{Context, Poll, ready},
};
use trillium_http::{
    Body, BodySource, Headers, HttpConfig, MutCow, ReceivedBody, ReceivedBodyState,
};
use trillium_server_common::{Runtime, Transport, url::Origin};

/// A response body received from a server.
///
/// Most of the time this represents a body that will be read from the underlying transport, but it
/// can also wrap an override body installed by middleware via [`ConnExt::set_response_body`]
/// — e.g. cache hits, mocked responses, or circuit-breaker short-circuits. Reads, encoding
/// handling, and `max_len` enforcement work transparently across both cases.
///
/// [`ConnExt::set_response_body`]: crate::ConnExt::set_response_body
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
/// [`ResponseBody::set_max_len`]. The bound is enforced on override bodies as well as
/// transport-backed ones, so a user-set memory cap holds even when middleware has replaced the
/// body with externally-sourced bytes.
pub struct ResponseBody<'a> {
    inner: ResponseBodyInner<'a>,
    /// Set on `'static` Received bodies built via
    /// [`Conn::take_response_body`][crate::Conn::take_response_body]. `recycle` and `Drop`
    /// consult it to decide whether to drain (keepalive) or close the underlying transport.
    /// `None` for borrowed bodies (the conn handles their cleanup) and for Override bodies (no
    /// transport is attached at this layer — `take_response_body` already evicted any leftover
    /// transport before returning).
    cleanup: Option<CleanupContext>,
}

#[allow(clippy::large_enum_variant)]
enum ResponseBodyInner<'a> {
    Received(ReceivedBody<'a, Box<dyn Transport>>),
    Override(OverrideBody<'a>),
    Closing(Pin<Box<dyn Future<Output = ()> + Send + Sync + 'static>>),
    Closed,
}

type H1Pool = Pool<Origin, Box<dyn Transport>>;

/// Carries everything `Drop for ResponseBody` and [`ResponseBody::recycle`] need to release
/// a transport without re-deriving from a [`crate::Conn`] (which is gone by then).
///
/// `pool_origin: Some` means "keepalive transport, pool is configured — insert here on
/// completion." `None` means "close on completion" (non-keepalive *or* no pool). The same
/// instance is cloned into the body's `on_completion` callback in
/// [`Conn::take_received_body`][crate::Conn::take_received_body], so the user-driven
/// drain/read paths and the Drop/recycle paths share one source of truth for what to do
/// with the transport when the body finishes.
#[derive(Clone)]
pub(crate) struct CleanupContext {
    pub(crate) runtime: Runtime,
    pub(crate) h1_pool_origin: Option<(H1Pool, Origin)>,
}

impl CleanupContext {
    /// Hand a freshly-released transport off to its destination — pool insert (sync) or
    /// spawn close.
    pub(crate) fn handoff(&self, mut transport: Box<dyn Transport>) {
        match &self.h1_pool_origin {
            Some((pool, origin)) => {
                log::trace!("body transferred, returning to pool");
                pool.insert(origin.clone(), PoolEntry::new(transport, None));
            }
            None => {
                self.runtime.clone().spawn(async move {
                    let _ = transport.close().await;
                });
            }
        }
    }
}

pub(crate) struct OverrideBody<'a> {
    body: MutCow<'a, Body>,
    encoding: &'static Encoding,
    max_len: u64,
    initial_len: usize,
    max_preallocate: usize,
}

impl AsyncRead for OverrideBody<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let remaining = self.max_len.saturating_sub(self.body.bytes_read());
        if remaining == 0 && !buf.is_empty() {
            return Poll::Ready(Err(io::Error::other(Error::ReceivedBodyTooLong(
                self.max_len,
            ))));
        }
        let cap = remaining.min(buf.len() as u64) as usize;
        Pin::new(&mut *self.body).poll_read(cx, &mut buf[..cap])
    }
}

impl<'a> OverrideBody<'a> {
    pub(crate) fn new(
        body: impl Into<MutCow<'a, Body>>,
        encoding: &'static Encoding,
        http_config: &HttpConfig,
    ) -> Self {
        Self {
            body: body.into(),
            encoding,
            max_len: http_config.received_body_max_len(),
            max_preallocate: http_config.received_body_max_preallocate(),
            initial_len: http_config.received_body_initial_len(),
        }
    }
}

impl Debug for ResponseBody<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.inner {
            ResponseBodyInner::Received(rb) => f.debug_tuple("ResponseBody").field(rb).finish(),
            ResponseBodyInner::Override(o) => f
                .debug_struct("ResponseBody (override)")
                .field("body", &*o.body)
                .field("encoding", &o.encoding.name())
                .field("max_len", &o.max_len)
                .finish(),
            ResponseBodyInner::Closing(_) => f.debug_tuple("ResponseBody (closing)").finish(),
            ResponseBodyInner::Closed => f.debug_tuple("ResponseBody (closed)").finish(),
        }
    }
}

impl AsyncRead for ResponseBody<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut bytes = 0;
        loop {
            match &mut self.inner {
                ResponseBodyInner::Received(rb) => bytes = ready!(Pin::new(rb).poll_read(cx, buf))?,
                ResponseBodyInner::Override(o) => bytes = ready!(Pin::new(o).poll_read(cx, buf))?,
                ResponseBodyInner::Closing(fut) => {
                    ready!(fut.as_mut().poll(cx));
                    self.inner = ResponseBodyInner::Closed;
                    break;
                }

                ResponseBodyInner::Closed => break,
            };

            // Inline transport settlement — see take_received_body's `cleanup` param for
            // why this isn't done via on_completion.
            if bytes == 0
                && let Some((mut rb, cleanup)) = self.prepare_for_recycle()
                && rb.state() == ReceivedBodyState::End
                && let Some(mut transport) = rb.take_transport()
            {
                if let Some((pool, origin)) = cleanup.h1_pool_origin {
                    pool.insert(origin, PoolEntry::new(transport, None));
                } else {
                    self.inner = ResponseBodyInner::Closing(Box::pin(async move {
                        if let Err(e) = transport.close().await {
                            log::warn!("transport close failed during ResponseBody EOF: {e}");
                        }
                    }));
                }
            } else {
                break;
            }
        }

        Poll::Ready(Ok(bytes))
    }
}

impl ResponseBody<'_> {
    fn take_inner(&mut self) -> ResponseBodyInner<'_> {
        mem::replace(&mut self.inner, ResponseBodyInner::Closed)
    }

    fn max_preallocate(&self) -> usize {
        match &self.inner {
            ResponseBodyInner::Received(rb) => rb.max_preallocate(),
            ResponseBodyInner::Override(override_body) => override_body.max_preallocate,
            _ => 0,
        }
    }

    fn max_len(&self) -> u64 {
        match &self.inner {
            ResponseBodyInner::Received(rb) => rb.max_len(),
            ResponseBodyInner::Override(override_body) => override_body.max_len,
            _ => 0,
        }
    }

    fn initial_len(&self) -> usize {
        match &self.inner {
            ResponseBodyInner::Received(rb) => rb.initial_len(),
            ResponseBodyInner::Override(override_body) => override_body.initial_len,
            _ => 0,
        }
    }

    fn encoding(&self) -> &'static Encoding {
        match &self.inner {
            ResponseBodyInner::Received(rb) => rb.encoding(),
            ResponseBodyInner::Override(override_body) => override_body.encoding,
            _ => encoding_rs::WINDOWS_1252,
        }
    }

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
    pub async fn read_bytes(mut self) -> Result<Vec<u8>, Error> {
        let mut vec = if let Some(len) = self.content_length() {
            if len > self.max_len() {
                return Err(Error::ReceivedBodyTooLong(self.max_len()));
            }

            let len =
                usize::try_from(len).map_err(|_| Error::ReceivedBodyTooLong(self.max_len()))?;

            Vec::with_capacity(len.min(self.max_preallocate()))
        } else {
            Vec::with_capacity(self.initial_len())
        };

        self.read_to_end(&mut vec).await?;

        Ok(vec)
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
        let encoding = self.encoding();
        let bytes = self.read_bytes().await?;
        let (s, _, _) = encoding.decode(&bytes);
        Ok(s.to_string())
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
        match &mut self.inner {
            ResponseBodyInner::Received(rb) => {
                rb.set_max_len(max_len);
            }
            ResponseBodyInner::Override(o) => {
                o.max_len = max_len;
            }
            _ => {}
        }
        self
    }

    /// The content-length of this body, if available.
    ///
    /// This value usually is derived from the content-length header. If the request that this body
    /// is attached to uses transfer-encoding chunked, this will be None.
    pub fn content_length(&self) -> Option<u64> {
        match &self.inner {
            ResponseBodyInner::Received(rb) => rb.content_length(),
            ResponseBodyInner::Override(o) => o.body.len(),
            _ => None,
        }
    }

    fn prepare_for_recycle(
        &mut self,
    ) -> Option<(
        ReceivedBody<'static, Box<dyn Transport + 'static>>,
        CleanupContext,
    )> {
        let cleanup = self.cleanup.take()?;

        let ResponseBodyInner::Received(rb) = self.take_inner() else {
            return None;
        };

        let rb = rb.try_into_owned()?;

        Some((rb, cleanup))
    }
}

// local &mut version of trillium-http's drain
async fn drain(rb: &mut ReceivedBody<'static, Box<dyn Transport + 'static>>) -> io::Result<u64> {
    let copy_loops_per_yield = rb.copy_loops_per_yield();
    trillium_http::copy(rb, futures_lite::io::sink(), copy_loops_per_yield).await
}

async fn recycle(
    mut rb: ReceivedBody<'static, Box<dyn Transport + 'static>>,
    h1_pool_origin: Option<(H1Pool, Origin)>,
) {
    if let Some((pool, origin)) = h1_pool_origin {
        match drain(&mut rb).await {
            Ok(drained) => {
                if rb.state() == ReceivedBodyState::End
                    && let Some(transport) = rb.take_transport()
                {
                    log::trace!(
                        "drained {drained} bytes, returning transport to pool for {origin:?}"
                    );
                    pool.insert(origin, PoolEntry::new(transport, None));
                    return;
                }
            }
            Err(e) => log::warn!("drain failed during recycle: {e}"),
        }
    }

    if let Some(mut transport) = rb.take_transport()
        && let Err(e) = transport.close().await
    {
        log::warn!("transport close failed during recycle: {e}");
    }
}

impl Drop for ResponseBody<'_> {
    fn drop(&mut self) {
        let Some((mut rb, cleanup)) = self.prepare_for_recycle() else {
            return;
        };

        // fast sync path for reclaiming an owned http/1.1 received body that's at End
        if rb.state() == ReceivedBodyState::End
            && cleanup.h1_pool_origin.is_some()
            && let Some(transport) = rb.take_transport()
            && let Some((pool, origin)) = cleanup.h1_pool_origin
        {
            pool.insert(origin, PoolEntry::new(transport, None));
        } else {
            cleanup.runtime.spawn(recycle(rb, cleanup.h1_pool_origin));
        }
    }
}

impl BodySource for ResponseBody<'static> {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        match &mut self.get_mut().inner {
            ResponseBodyInner::Received(rb) => Pin::new(rb).trailers(),
            ResponseBodyInner::Override(o) => o.body.trailers(),
            _ => None,
        }
    }
}

impl<'a> From<ReceivedBody<'a, Box<dyn Transport>>> for ResponseBody<'a> {
    fn from(received_body: ReceivedBody<'a, Box<dyn Transport>>) -> Self {
        Self {
            inner: ResponseBodyInner::Received(received_body),
            cleanup: None,
        }
    }
}

impl<'a> From<OverrideBody<'a>> for ResponseBody<'a> {
    fn from(o: OverrideBody<'a>) -> Self {
        Self {
            inner: ResponseBodyInner::Override(o),
            cleanup: None,
        }
    }
}

impl ResponseBody<'static> {
    pub(crate) fn received_owned(
        body: ReceivedBody<'static, Box<dyn Transport>>,
        cleanup: CleanupContext,
    ) -> Self {
        Self {
            inner: ResponseBodyInner::Received(body),
            cleanup: Some(cleanup),
        }
    }

    /// Drains and pools the underlying transport when worthwhile, closes it otherwise.
    ///
    /// Use this to release a keepalive transport synchronously before reissuing a request on
    /// the same client — the redirect/retry handler pattern. For an h1.1 keepalive transport
    /// this drives the body to EOF and returns the transport to the pool. For a non-keepalive
    /// transport this calls `transport.close()` directly without draining (since draining
    /// would just waste bytes on a connection we're about to close).
    ///
    /// For an Override body (cache hit, mocked response, tee), this is a no-op — the body's
    /// own components handle their own teardown when dropped.
    pub async fn recycle(mut self) {
        let Some((rb, cleanup)) = self.prepare_for_recycle() else {
            return;
        };

        recycle(rb, cleanup.h1_pool_origin).await;
    }
}

impl<'a> IntoFuture for ResponseBody<'a> {
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send + 'a>>;
    type Output = trillium_http::Result<String>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.read_string().await })
    }
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<ResponseBody<'static>>();
    assert_send_sync::<ResponseBody<'_>>();
};
