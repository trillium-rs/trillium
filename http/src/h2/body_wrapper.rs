use crate::{Body, Headers, body::BodyType};
use futures_lite::AsyncRead;
use std::{
    any::Any,
    io, mem,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    task::Poll,
};

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
/// The driver polls and drops every body it sends on its own task, so a panicking body source
/// would otherwise unwind the driver and take down every stream on the connection. Source code —
/// `poll_read`, `trailers`, and its drop — runs under `catch_unwind`, and every panic is logged. A
/// panic in `poll_read` or `trailers` becomes an error, which resets just this stream; one in its
/// drop leaves the stream's outcome as it was.
#[derive(Debug)]
pub(crate) struct H2Body {
    body: BodyType,
    len: Option<u64>,
    panicked: bool,
}

impl H2Body {
    pub(crate) fn new(body: Body) -> Self {
        Self {
            len: body.len(),
            body: body.0,
            panicked: false,
        }
    }

    /// The body's declared length, if it has one.
    pub(crate) fn len(&self) -> Option<u64> {
        self.len
    }

    /// Returns trailers from the body source, if any. Only meaningful after the body has
    /// been fully read (`poll_read` returned `Ok(0)` and set the inner `done` flag).
    pub(crate) fn trailers(&mut self) -> io::Result<Option<Headers>> {
        match &mut self.body {
            BodyType::Streaming {
                async_read, done, ..
            } if *done => contain_panic(|| async_read.get_mut().as_mut().trailers()),
            _ => Ok(None),
        }
    }
}

impl Drop for H2Body {
    fn drop(&mut self) {
        let body = mem::replace(&mut self.body, BodyType::Empty);
        let _ = contain_panic(|| drop(body));
    }
}

/// Run body-source code, turning a panic into a logged error. Asserting unwind safety is sound
/// here because a body that panicked is never polled again; only its drop still runs, and that is
/// contained too.
fn contain_panic<T>(f: impl FnOnce() -> T) -> io::Result<T> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|payload| {
        let message = panic_message(&*payload);
        log::error!("h2 body panicked: {message}");
        io::Error::other(format!("h2 body panicked: {message}"))
    })
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload")
}

impl AsyncRead for H2Body {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.panicked {
            return Poll::Ready(Err(io::Error::other("h2 body panicked earlier")));
        }
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
                let polled = contain_panic(|| async_read.get_mut().as_mut().poll_read(cx, buf))
                    .inspect_err(|_| this.panicked = true)?;
                let bytes = futures_lite::ready!(polled)?;
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
