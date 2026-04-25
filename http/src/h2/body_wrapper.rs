use crate::{Body, Headers, body::BodyType};
use futures_lite::AsyncRead;
use std::{io, pin::Pin, task::Poll};

/// h2 view over a [`Body`] that bypasses the chunked-transfer-encoding [`Body`] applies on
/// `BodyType::Streaming { len: None }`.
///
/// h2 frames DATA at the connection layer (in the driver's send pump), not in the body —
/// so the bytes the body yields must be plain payload, not chunk-encoded. `H2Body` peels
/// off the chunking by delegating directly to the inner [`BodySource`][crate::BodySource]'s
/// `poll_read` for streaming bodies, and copies-out for static bodies. Trailers are
/// preserved: [`H2Body::trailers`] forwards to the inner `BodySource::trailers` once the
/// stream is fully drained, so the send pump's trailing-HEADERS emission still works.
///
/// Mirrors [`H3Body`][crate::h3::H3Body] in role, minus the per-chunk DATA frame header
/// (h2's send pump frames DATA itself; h3 prepends frame headers inside the body).
#[derive(Debug)]
pub(crate) struct H2Body {
    body: BodyType,
}

impl H2Body {
    pub(crate) fn new(body: Body) -> Self {
        Self { body: body.0 }
    }

    /// Returns trailers from the body source, if any. Only meaningful after the body has
    /// been fully read (`poll_read` returned `Ok(0)` and set the inner `done` flag).
    pub(crate) fn trailers(&mut self) -> Option<Headers> {
        match &mut self.body {
            BodyType::Streaming {
                async_read, done, ..
            } if *done => async_read.get_mut().as_mut().trailers(),
            _ => None,
        }
    }
}

impl AsyncRead for H2Body {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match &mut this.body {
            BodyType::Empty => Poll::Ready(Ok(0)),

            BodyType::Static { content, cursor } => {
                let remaining = content.len() - *cursor;
                if remaining == 0 {
                    return Poll::Ready(Ok(0));
                }
                let bytes = remaining.min(buf.len());
                buf[..bytes].copy_from_slice(&content[*cursor..*cursor + bytes]);
                *cursor += bytes;
                Poll::Ready(Ok(bytes))
            }

            BodyType::Streaming {
                async_read,
                done,
                progress,
                ..
            } => {
                if *done {
                    return Poll::Ready(Ok(0));
                }
                let bytes = futures_lite::ready!(async_read.get_mut().as_mut().poll_read(cx, buf))?;
                if bytes == 0 {
                    *done = true;
                } else {
                    *progress += bytes as u64;
                }
                Poll::Ready(Ok(bytes))
            }
        }
    }
}
