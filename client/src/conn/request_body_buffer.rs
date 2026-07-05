//! Buffering a streaming request body up to a size limit, so the h1 send path can frame it
//! precisely when the whole body fits — with an accurate `Content-Length` and no
//! `Expect: 100-continue` handshake. See [`buffer_request_body`].

use futures_lite::{AsyncRead, AsyncReadExt};
use std::{
    io::Result,
    pin::Pin,
    task::{Context, Poll},
};
use trillium_http::{Body, BodySource, Headers};

/// Read up to `limit` bytes of `body` into memory, returning the (possibly rewrapped) body and
/// whether it was *fully buffered* — i.e. reached EOF within `limit`.
///
/// - Fully buffered, no trailers → a fixed-length static body, framed with `Content-Length`.
/// - Fully buffered *with* trailers → a streaming body that replays the buffered bytes and then
///   yields the trailers; it must be sent chunked, since trailers require chunked framing.
/// - Overflowed `limit` → a streaming body that replays the buffered prefix and then continues from
///   the original source.
///
/// A fully-buffered body is cheap to send in one shot, so the caller can skip the
/// `Expect: 100-continue` handshake; only a body that overflowed `limit` benefits from it.
/// Static, empty, or already-known-length bodies are returned unchanged with `true`.
pub(crate) async fn buffer_request_body(body: Body, limit: usize) -> Result<(Body, bool)> {
    if body.len().is_some() {
        return Ok((body, true));
    }

    let Some(mut source) = body.into_body_source() else {
        // A body with no known length is always streaming, so this is unreachable; fall back to
        // an empty body rather than panic.
        return Ok((Body::default(), true));
    };

    // One byte past `limit` distinguishes "fits" from "overflow": filling this buffer without
    // hitting EOF means the body is larger than we are willing to buffer.
    let mut buf = vec![0u8; limit + 1];
    let mut filled = 0;
    let fully_buffered = loop {
        if filled == buf.len() {
            break false;
        }
        let bytes = source.read(&mut buf[filled..]).await?;
        if bytes == 0 {
            break true;
        }
        filled += bytes;
    };
    buf.truncate(filled);

    let body = if !fully_buffered {
        Body::new_with_trailers(
            PrefixedBody {
                prefix: buf,
                cursor: 0,
                inner: source,
            },
            None,
        )
    } else if let Some(trailers) = source.as_mut().trailers() {
        Body::new_with_trailers(
            BufferedTrailers {
                content: buf,
                cursor: 0,
                trailers: Some(trailers),
            },
            None,
        )
    } else {
        Body::new_static(buf)
    };

    Ok((body, fully_buffered))
}

/// Replays a buffered prefix, then continues from the original source (the overflow case).
struct PrefixedBody {
    prefix: Vec<u8>,
    cursor: usize,
    inner: Pin<Box<dyn BodySource>>,
}

impl AsyncRead for PrefixedBody {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        let this = self.get_mut();
        if this.cursor < this.prefix.len() {
            let n = buf.len().min(this.prefix.len() - this.cursor);
            buf[..n].copy_from_slice(&this.prefix[this.cursor..this.cursor + n]);
            this.cursor += n;
            return Poll::Ready(Ok(n));
        }
        this.inner.as_mut().poll_read(cx, buf)
    }
}

impl BodySource for PrefixedBody {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        self.get_mut().inner.as_mut().trailers()
    }
}

/// Replays fully-buffered content, then yields trailers (the small-body-with-trailers case).
struct BufferedTrailers {
    content: Vec<u8>,
    cursor: usize,
    trailers: Option<Headers>,
}

impl AsyncRead for BufferedTrailers {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        let this = self.get_mut();
        let n = buf.len().min(this.content.len() - this.cursor);
        buf[..n].copy_from_slice(&this.content[this.cursor..this.cursor + n]);
        this.cursor += n;
        Poll::Ready(Ok(n))
    }
}

impl BodySource for BufferedTrailers {
    fn trailers(self: Pin<&mut Self>) -> Option<Headers> {
        self.get_mut().trailers.take()
    }
}
