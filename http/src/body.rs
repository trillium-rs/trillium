use crate::{Headers, h2::H2Body, h3::H3Body};
use BodyType::{Empty, Static, Streaming};
use futures_lite::{AsyncRead, AsyncReadExt, io::Cursor, ready};
use pin_project_lite::pin_project;
use std::{
    borrow::Cow,
    fmt::{self, Debug, Formatter},
    io::{Error, Result},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use sync_wrapper::SyncWrapper;

/// Streaming body source that can optionally produce trailers.
///
/// Implement this on types that compute trailer headers dynamically as the body
/// is read — for example, a hashing wrapper that produces a `Digest` trailer
/// after all bytes have been streamed. For plain [`AsyncRead`] sources with no
/// trailers, [`Body::new_streaming`] is simpler.
pub trait BodySource: AsyncRead + Send + 'static {
    /// Returns the trailers for this body, called after the body has been fully read.
    ///
    /// Implementations may clear internal state on this call; the result is
    /// only meaningful after [`AsyncRead::poll_read`] has returned `Ok(0)`.
    fn trailers(self: Pin<&mut Self>) -> Option<Headers>;
}

pin_project! {
    struct PlainBody<T> {
        #[pin]
        async_read: T,
    }
}

impl<T: AsyncRead> AsyncRead for PlainBody<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        self.project().async_read.poll_read(cx, buf)
    }
}

impl<T: AsyncRead + Send + 'static> BodySource for PlainBody<T> {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        None
    }
}

/// The trillium representation of a http body. This can contain
/// either `&'static [u8]` content, `Vec<u8>` content, or a boxed
/// [`AsyncRead`]/[`BodySource`] type.
#[derive(Debug, Default)]
pub struct Body(pub(crate) BodyType);

impl Body {
    /// Construct a new body from a streaming [`AsyncRead`] source. If
    /// you have the body content in memory already, prefer
    /// [`Body::new_static`] or one of the From conversions.
    pub fn new_streaming(async_read: impl AsyncRead + Send + 'static, len: Option<u64>) -> Self {
        Self::new_with_trailers(PlainBody { async_read }, len)
    }

    /// Construct a new body from a [`BodySource`] that can produce trailers after
    /// the body has been fully read.
    ///
    /// Use this when trailers must be computed dynamically from the body bytes,
    /// for example to append a content hash.
    pub fn new_with_trailers(body: impl BodySource, len: Option<u64>) -> Self {
        Self(Streaming {
            async_read: SyncWrapper::new(Box::pin(body)),
            len,
            done: false,
            progress: 0,
            chunked_framing: true,
            keep_open: false,
        })
    }

    /// Disable chunked-encoding framing emitted by [`AsyncRead`] for streaming bodies
    /// of unknown length.
    ///
    /// By default, when a streaming body has no known length, this type's [`AsyncRead`]
    /// implementation emits chunked framing so the h1 codec can write its bytes directly.
    /// That framing is wrong for any consumer that wants raw body bytes.
    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    #[must_use]
    pub fn without_chunked_framing(mut self) -> Self {
        if let Streaming {
            ref mut chunked_framing,
            ..
        } = self.0
        {
            *chunked_framing = false;
        }
        self
    }

    /// Normalize this body to an open, chunk-framed stream: its content goes out as
    /// chunked-transfer chunks with **no** terminating `0\r\n`, leaving the outbound
    /// stream open for a following upgrade to continue and eventually close.
    ///
    /// Fixed-length content (`Static`, or a streaming body with a known length) is
    /// re-sourced through the chunked path so it too flows as ordinary chunks rather than
    /// raw bytes — the caller doesn't have to hand-wrap it in a length-less streaming body.
    /// An empty body stays empty (it contributes no bytes; the upgrade owns the whole
    /// stream).
    ///
    /// The send site that consumes the body is responsible for *not* writing the
    /// trailer-section terminator either; trailers (if any) ride onto the upgrade.
    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    #[must_use]
    pub fn keep_open(mut self) -> Self {
        self.set_keep_open();
        self
    }

    /// In-crate counterpart to [`keep_open`](Self::keep_open) — the server send path sets
    /// this from `should_upgrade()` and can't reach the `unstable`-gated public builder.
    pub(crate) fn set_keep_open(&mut self) {
        // Re-source fixed content through the chunked streaming path so it goes out as
        // chunks instead of raw bytes. Streaming bodies are left in place (preserving their
        // `BodySource`, hence any trailers) and just have their framing flags flipped below.
        if matches!(self.0, Static { .. }) {
            let reader = std::mem::take(self).into_reader();
            *self = Self::new_streaming(reader, None);
        }

        if let Streaming {
            ref mut len,
            ref mut chunked_framing,
            ref mut keep_open,
            ..
        } = self.0
        {
            *len = None;
            *chunked_framing = true;
            *keep_open = true;
        }
    }

    /// Set whether this body's [`AsyncRead`] impl emits chunked framing for the
    /// `len: None` case. The h1 send path drives this from the finalized response
    /// headers: `chunked` when `Transfer-Encoding: chunked` is present, raw passthrough
    /// (close-delimited) when neither it nor `Content-Length` is. Has no effect on
    /// fixed-length or static bodies, which never chunk.
    pub(crate) fn set_chunked_framing(&mut self, on: bool) {
        if let Streaming {
            ref mut chunked_framing,
            ..
        } = self.0
        {
            *chunked_framing = on;
        }
    }

    /// Returns trailers from the body source, if any.
    ///
    /// Only meaningful after the body has been fully read (i.e., [`AsyncRead::poll_read`]
    /// has returned `Ok(0)`). Returns `None` for bodies constructed with
    /// [`Body::new_streaming`] or [`Body::new_static`].
    #[doc(hidden)]
    pub fn trailers(&mut self) -> Option<Headers> {
        match &mut self.0 {
            Streaming {
                async_read, done, ..
            } if *done => async_read.get_mut().as_mut().trailers(),
            _ => None,
        }
    }

    /// Construct a fixed-length Body from a `Vec<u8>` or `&'static
    /// [u8]`.
    pub fn new_static(content: impl Into<Cow<'static, [u8]>>) -> Self {
        Self(Static {
            content: StaticContent::Cow(content.into()),
            cursor: 0,
        })
    }

    /// Retrieve a borrow of the static content in this body. If this
    /// body is a streaming body or an empty body, this will return
    /// None.
    pub fn static_bytes(&self) -> Option<&[u8]> {
        match &self.0 {
            Static { content, .. } => Some(content.as_ref()),
            _ => None,
        }
    }

    /// Transform this Body into a dyn [`AsyncRead`], wrapping static content in
    /// a [`Cursor`]. Unlike reading from the Body directly, this does not apply
    /// chunked encoding.
    pub fn into_reader(self) -> Pin<Box<dyn AsyncRead + Send + Sync + 'static>> {
        match self.0 {
            Streaming { async_read, .. } => Box::pin(SyncAsyncReader(async_read)),
            Static { content, .. } => Box::pin(Cursor::new(content)),
            Empty => Box::pin(Cursor::new("")),
        }
    }

    /// Consume this body and return the full content. If the body was constructed
    /// with [`Body::new_streaming`], this will read the entire streaming body into
    /// memory, awaiting the streaming source's completion.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying transport errors, or if a streaming body
    /// has already been partially or fully read.
    pub async fn into_bytes(self) -> Result<Cow<'static, [u8]>> {
        match self.0 {
            Static { content, .. } => Ok(content.into_cow()),

            Streaming {
                async_read,
                len,
                progress: 0,
                done: false,
                ..
            } => {
                let mut async_read = async_read.into_inner();
                let mut buf = len
                    .and_then(|c| c.try_into().ok())
                    .map(Vec::with_capacity)
                    .unwrap_or_default();

                async_read.read_to_end(&mut buf).await?;

                Ok(Cow::Owned(buf))
            }

            Empty => Ok(Cow::Borrowed(b"")),

            Streaming { .. } => Err(Error::other("body already read to completion")),
        }
    }

    /// Retrieve the number of bytes that have been read from this
    /// body
    pub fn bytes_read(&self) -> u64 {
        self.0.bytes_read()
    }

    /// returns the content length of this body, if known and
    /// available.
    pub fn len(&self) -> Option<u64> {
        self.0.len()
    }

    /// determine if the this body represents no data
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// determine if the this body represents static content
    pub fn is_static(&self) -> bool {
        matches!(self.0, Static { .. })
    }

    /// determine if the this body represents streaming content
    pub fn is_streaming(&self) -> bool {
        matches!(self.0, Streaming { .. })
    }

    /// Consume this body and return its underlying [`BodySource`], if it is a streaming body.
    ///
    /// Returns `None` for static and empty bodies, whose content is already in memory and whose
    /// length is already known. This is the extraction point a sender needs to buffer a
    /// streaming body while preserving its trailer-producing source — unlike
    /// [`into_reader`](Self::into_reader), which erases the source to a plain `AsyncRead`.
    #[cfg(feature = "unstable")]
    #[doc(hidden)]
    pub fn into_body_source(self) -> Option<Pin<Box<dyn BodySource>>> {
        match self.0 {
            Streaming { async_read, .. } => Some(async_read.into_inner()),
            _ => None,
        }
    }

    /// Attempt to clone this body. Returns `None` for streaming bodies, which are one-shot.
    ///
    /// Static bodies clone cheaply — a `Cow` clone, which is a pointer copy for borrowed
    /// `&'static` content and a `Vec` clone for owned content. The clone resets read
    /// progress, so it can be sent again from the beginning. Empty bodies always clone
    /// successfully.
    #[doc(hidden)]
    #[cfg(feature = "unstable")]
    pub fn try_clone(&self) -> Option<Self> {
        match &self.0 {
            Empty => Some(Self::default()),
            Static { content, .. } => Some(Self(Static {
                content: content.clone(),
                cursor: 0,
            })),
            Streaming { .. } => None,
        }
    }

    /// Convert this body into an `H3Body` for reading
    #[cfg(feature = "unstable")]
    pub fn into_h3(self) -> H3Body {
        H3Body::new(self)
    }

    /// Convert this body into an `H3Body` for reading
    #[cfg(not(feature = "unstable"))]
    pub(crate) fn into_h3(self) -> H3Body {
        H3Body::new(self)
    }

    /// Convert this body into an [`H2Body`] for reading by the h2 send pump.
    ///
    /// h2 frames DATA at the connection layer, so the body bytes that reach the send pump
    /// must be plain payload — not chunk-encoded. [`H2Body`] strips the chunked-transfer
    /// wrapping that [`Body::poll_read`] applies for the h1 path on streaming bodies of
    /// unknown length, and forwards trailers so the send pump can emit trailing HEADERS.
    pub(crate) fn into_h2(self) -> H2Body {
        H2Body::new(self)
    }
}

#[allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    reason = "buffers are well below petabyte scale; log2/4 of a usize stays in f64 range, and \
              the subtraction always yields a non-negative usize-representable value"
)]
fn max_bytes_to_read(buf_len: usize) -> usize {
    assert!(
        buf_len >= 6,
        "buffers of length {buf_len} are too small for this implementation.
            if this is a problem for you, please open an issue"
    );

    let bytes_remaining_after_two_cr_lns = (buf_len - 4) as f64;
    // maximum number of bytes the hex representation of the remaining bytes might take
    let max_bytes_of_hex_framing = (bytes_remaining_after_two_cr_lns).log2() / 4f64;
    (bytes_remaining_after_two_cr_lns - max_bytes_of_hex_framing.ceil()) as usize
}

impl AsyncRead for Body {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            Empty => Poll::Ready(Ok(0)),
            Static { content, cursor } => {
                let length = content.len();
                if length == *cursor {
                    return Poll::Ready(Ok(0));
                }
                let bytes = (length - *cursor).min(buf.len());
                buf[0..bytes].copy_from_slice(&content[*cursor..*cursor + bytes]);
                *cursor += bytes;
                Poll::Ready(Ok(bytes))
            }

            Streaming {
                async_read,
                len: Some(len),
                done,
                progress,
                ..
            } => {
                if *done {
                    return Poll::Ready(Ok(0));
                }

                let max_bytes_to_read = (*len - *progress)
                    .try_into()
                    .unwrap_or(buf.len())
                    .min(buf.len());

                let bytes = ready!(
                    async_read
                        .get_mut()
                        .as_mut()
                        .poll_read(cx, &mut buf[..max_bytes_to_read])
                )?;

                if bytes == 0 {
                    *done = true;
                } else {
                    *progress += bytes as u64;
                }

                Poll::Ready(Ok(bytes))
            }

            Streaming {
                async_read,
                len: None,
                done,
                progress,
                chunked_framing,
                keep_open,
            } => {
                if *done {
                    return Poll::Ready(Ok(0));
                }

                if !*chunked_framing {
                    let bytes = ready!(async_read.get_mut().as_mut().poll_read(cx, buf))?;
                    if bytes == 0 {
                        *done = true;
                    } else {
                        *progress += bytes as u64;
                    }
                    return Poll::Ready(Ok(bytes));
                }

                let max_bytes_to_read = max_bytes_to_read(buf.len());

                let bytes = ready!(
                    async_read
                        .get_mut()
                        .as_mut()
                        .poll_read(cx, &mut buf[..max_bytes_to_read])
                )?;

                if bytes == 0 {
                    *done = true;
                    if *keep_open {
                        // The outbound stream continues into an upgrade; the upgrade owns
                        // the terminator. Emit no last-chunk marker.
                        return Poll::Ready(Ok(0));
                    }
                    // Last-chunk marker only; the caller emits the trailer-section
                    // (possibly empty) followed by the terminating `\r\n`. Trailers come
                    // from `BodySource::trailers()` as structured `Headers`, not bytes,
                    // and the caller writes them in one shot so this path doesn't need
                    // a multi-poll state machine spanning buffers.
                    buf[..3].copy_from_slice(b"0\r\n");
                    return Poll::Ready(Ok(3));
                }

                *progress += bytes as u64;

                let start = format!("{bytes:X}\r\n");
                let start_length = start.len();
                let total = bytes + start_length + 2;
                buf.copy_within(..bytes, start_length);
                buf[..start_length].copy_from_slice(start.as_bytes());
                buf[total - 2..total].copy_from_slice(b"\r\n");
                Poll::Ready(Ok(total))
            }
        }
    }
}

struct SyncAsyncReader(SyncWrapper<Pin<Box<dyn BodySource>>>);
impl Debug for SyncAsyncReader {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncAsyncReader").finish()
    }
}
impl AsyncRead for SyncAsyncReader {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        self.get_mut().0.get_mut().as_mut().poll_read(cx, buf)
    }
}

/// In-memory fixed-length body content. Each variant is cheap to clone: borrowed and
/// shared variants copy a pointer, and the owned `Cow` variant clones its `Vec`.
#[derive(Clone)]
pub(crate) enum StaticContent {
    Cow(Cow<'static, [u8]>),
    Bytes(Arc<[u8]>),
    Str(Arc<str>),
}

impl std::ops::Deref for StaticContent {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            StaticContent::Cow(content) => content,
            StaticContent::Bytes(content) => content,
            StaticContent::Str(content) => content.as_bytes(),
        }
    }
}

impl AsRef<[u8]> for StaticContent {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl StaticContent {
    /// Materialize as an owned `Cow`. The `Cow` variant passes through without copying;
    /// the shared variants copy their bytes into a `Vec`.
    fn into_cow(self) -> Cow<'static, [u8]> {
        match self {
            StaticContent::Cow(content) => content,
            other => Cow::Owned(other.to_vec()),
        }
    }
}

#[derive(Default)]
pub(crate) enum BodyType {
    #[default]
    Empty,

    Static {
        content: StaticContent,
        cursor: usize,
    },

    Streaming {
        async_read: SyncWrapper<Pin<Box<dyn BodySource>>>,
        progress: u64,
        len: Option<u64>,
        done: bool,
        /// When true (the default), [`Body`]'s [`AsyncRead`] impl emits chunked
        /// framing for the `len: None` case; when false (via
        /// [`Body::without_chunked_framing`]), it passes through raw bytes.
        chunked_framing: bool,
        /// When true (via [`Body::keep_open`]), the chunked `len: None` read does not
        /// emit the `0\r\n` last-chunk marker at EOF — the outbound stream is left open
        /// for a following upgrade to terminate. Default false.
        keep_open: bool,
    },
}

impl Debug for BodyType {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Empty => f.debug_tuple("BodyType::Empty").finish(),
            Static { content, cursor } => f
                .debug_struct("BodyType::Static")
                .field("content", &String::from_utf8_lossy(content))
                .field("cursor", cursor)
                .finish(),
            Streaming {
                len,
                done,
                progress,
                ..
            } => f
                .debug_struct("BodyType::Streaming")
                .field("async_read", &format_args!(".."))
                .field("len", &len)
                .field("done", &done)
                .field("progress", &progress)
                .finish(),
        }
    }
}

impl BodyType {
    fn is_empty(&self) -> bool {
        match *self {
            Empty => true,
            Static { ref content, .. } => content.is_empty(),
            Streaming { len, .. } => len == Some(0),
        }
    }

    fn len(&self) -> Option<u64> {
        match *self {
            Empty => Some(0),
            Static { ref content, .. } => Some(content.len() as u64),
            Streaming { len, .. } => len,
        }
    }

    fn bytes_read(&self) -> u64 {
        match *self {
            Empty => 0,
            Static { cursor, .. } => cursor as u64,
            Streaming { progress, .. } => progress,
        }
    }
}

impl From<String> for Body {
    fn from(s: String) -> Self {
        s.into_bytes().into()
    }
}

impl From<&'static str> for Body {
    fn from(s: &'static str) -> Self {
        s.as_bytes().into()
    }
}

impl From<&'static [u8]> for Body {
    fn from(content: &'static [u8]) -> Self {
        Self::new_static(content)
    }
}

impl From<Vec<u8>> for Body {
    fn from(content: Vec<u8>) -> Self {
        Self::new_static(content)
    }
}

impl From<Cow<'static, [u8]>> for Body {
    fn from(value: Cow<'static, [u8]>) -> Self {
        Self::new_static(value)
    }
}

impl From<Cow<'static, str>> for Body {
    fn from(value: Cow<'static, str>) -> Self {
        match value {
            Cow::Borrowed(b) => b.into(),
            Cow::Owned(o) => o.into(),
        }
    }
}

impl From<Arc<[u8]>> for Body {
    fn from(content: Arc<[u8]>) -> Self {
        Self(Static {
            content: StaticContent::Bytes(content),
            cursor: 0,
        })
    }
}

impl From<Arc<str>> for Body {
    fn from(content: Arc<str>) -> Self {
        Self(Static {
            content: StaticContent::Str(content),
            cursor: 0,
        })
    }
}

#[cfg(test)]
mod test_shared_content {
    use super::Body;
    use futures_lite::future::block_on;
    use std::sync::Arc;

    #[test]
    fn arc_bytes_roundtrips() {
        let arc: Arc<[u8]> = Arc::from(&b"shared bytes"[..]);
        let body = Body::from(Arc::clone(&arc));
        assert_eq!(body.len(), Some(12));
        assert_eq!(body.static_bytes(), Some(&b"shared bytes"[..]));
        assert_eq!(
            block_on(body.into_bytes()).unwrap().as_ref(),
            b"shared bytes"
        );
        // the source Arc is still usable — the body shared, not consumed, the buffer
        assert_eq!(&*arc, b"shared bytes");
    }

    #[test]
    fn arc_str_roundtrips() {
        let arc: Arc<str> = Arc::from("shared str");
        let body = Body::from(arc);
        assert_eq!(body.len(), Some(10));
        assert_eq!(body.static_bytes(), Some(&b"shared str"[..]));
        assert_eq!(block_on(body.into_bytes()).unwrap().as_ref(), b"shared str");
    }

    #[cfg(feature = "unstable")]
    #[test]
    fn shared_body_clones_without_copying_the_arc() {
        let arc: Arc<[u8]> = Arc::from(&b"abc"[..]);
        let body = Body::from(Arc::clone(&arc));
        let clone = body.try_clone().expect("static bodies clone");
        assert_eq!(clone.static_bytes(), Some(&b"abc"[..]));
        // original + body + clone all reference the same allocation
        assert_eq!(Arc::strong_count(&arc), 3);
    }
}

#[cfg(test)]
mod test_bytes_to_read {
    #[test]
    fn simple_check_of_known_values() {
        // the marked rows are the most important part of this test,
        // and a nonobvious but intentional consequence of the
        // implementation. in order to avoid overflowing, we must use
        // one fewer than the available buffer bytes because
        // increasing the read size increase the number of framed
        // bytes by two. This occurs when the hex representation of
        // the content bytes is near an increase in order of magnitude
        // (F->10, FF->100, FFF-> 1000, etc)
        let values = vec![
            (6, 1),       // 1
            (7, 2),       // 2
            (20, 15),     // F
            (21, 15),     // F <-
            (22, 16),     // 10
            (23, 17),     // 11
            (260, 254),   // FE
            (261, 254),   // FE <-
            (262, 255),   // FF <-
            (263, 256),   // 100
            (4100, 4093), // FFD
            (4101, 4093), // FFD <-
            (4102, 4094), // FFE <-
            (4103, 4095), // FFF <-
            (4104, 4096), // 1000
        ];

        for (input, expected) in values {
            let actual = super::max_bytes_to_read(input);
            assert_eq!(
                actual, expected,
                "\n\nexpected max_bytes_to_read({input}) to be {expected}, but it was {actual}"
            );

            // testing the test:
            let used_bytes = expected + 4 + format!("{expected:X}").len();
            assert!(
                used_bytes == input || used_bytes == input - 1,
                "\n\nfor an input of {}, expected used bytes to be {} or {}, but was {}",
                input,
                input,
                input - 1,
                used_bytes
            );
        }
    }
}
