use crate::{Body, Headers, body::BodyType, h3::Frame};
use futures_lite::{AsyncRead, ready};
use std::{io, pin::Pin, task::Poll};

/// h3 view over a [`Body`] that prepends HTTP/3 DATA frame headers (RFC 9114 §7.2.1)
/// to the payload yielded by the inner [`BodyType`].
///
/// h3 frames DATA inline with the body bytes (unlike h2, which frames at the driver layer
/// in the send pump): each `poll_read` writes a varint-length DATA frame header followed
/// by payload into the caller's buffer. Known-length bodies emit one frame whose payload
/// spans the whole body and stream into it across polls; unknown-length bodies emit one
/// frame per `poll_read` (the per-frame length must be known when the header is written,
/// so we can't open a single frame ahead of time).
///
/// Mirrors [`H2Body`][crate::h2::H2Body] in role — both peel off the chunked-transfer
/// wrapping that [`Body::poll_read`] applies for the h1 path on streaming bodies of
/// unknown length — and adds the h3-specific DATA framing on top.
#[derive(Debug)]
pub struct H3Body {
    body: BodyType,
    /// Whether the single DATA frame header has been written. Only meaningful for
    /// known-length bodies, which open one frame whose payload spans the whole body
    /// and stream into it across polls. Stays false for unknown-length bodies (each
    /// poll opens a new frame, so there is no persistent "header already written" state).
    header_written: bool,
}

impl From<BodyType> for H3Body {
    fn from(body: BodyType) -> Self {
        Self {
            body,
            header_written: false,
        }
    }
}

impl H3Body {
    pub(crate) fn new(body: Body) -> Self {
        body.0.into()
    }

    /// Returns trailers from the body source, if any.
    ///
    /// Only meaningful after the body has been fully read (`done == true`).
    pub fn trailers(&mut self) -> Option<Headers> {
        match &mut self.body {
            BodyType::Streaming {
                async_read, done, ..
            } if *done => async_read.get_mut().as_mut().trailers(),
            _ => None,
        }
    }
}

impl AsyncRead for H3Body {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        // Each branch encodes the body as one or more HTTP/3 DATA frames (RFC 9114 §7.2.1).
        // Known-length bodies (Static, Streaming { len: Some(_) }) emit one DATA frame whose
        // payload length spans the whole body; later polls deliver more bytes into that
        // already-opened frame. Unknown-length bodies (Streaming { len: None }) emit one
        // DATA frame per poll — the per-frame length must be known when the header is
        // written, so we can't open a single frame ahead of time.
        match &mut this.body {
            BodyType::Empty => Poll::Ready(Ok(0)),

            BodyType::Static { content, cursor } => {
                let remaining = content.len() - *cursor;
                if remaining == 0 {
                    return Poll::Ready(Ok(0));
                }

                let mut written = 0;
                if !this.header_written {
                    let frame = Frame::Data(remaining as u64);
                    written += frame.encode(buf).ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::WriteZero,
                            "buffer too small for frame header",
                        )
                    })?;
                    this.header_written = true;
                }

                let bytes = remaining.min(buf.len() - written);
                buf[written..written + bytes].copy_from_slice(&content[*cursor..*cursor + bytes]);
                *cursor += bytes;
                Poll::Ready(Ok(written + bytes))
            }

            BodyType::Streaming {
                async_read,
                len: Some(len),
                done,
                progress,
                ..
            } => {
                if *done {
                    return Poll::Ready(Ok(0));
                }

                let header_len = if this.header_written {
                    0
                } else {
                    Frame::Data(*len).encoded_len()
                };

                let max_bytes = (*len - *progress)
                    .try_into()
                    .unwrap_or(buf.len() - header_len)
                    .min(buf.len() - header_len);

                let bytes = ready!(
                    async_read
                        .get_mut()
                        .as_mut()
                        .poll_read(cx, &mut buf[header_len..header_len + max_bytes])
                )?;

                if !this.header_written {
                    Frame::Data(*len).encode(buf);
                    this.header_written = true;
                }

                if bytes == 0 {
                    *done = true;
                } else {
                    *progress += bytes as u64;
                }

                Poll::Ready(Ok(header_len + bytes))
            }

            BodyType::Streaming {
                async_read,
                len: None,
                done,
                progress,
                ..
            } => {
                if *done {
                    return Poll::Ready(Ok(0));
                }

                let reserved = Frame::Data(buf.len() as u64).encoded_len();
                if buf.len() <= reserved {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::WriteZero,
                        "buffer too small for DATA frame",
                    )));
                }

                let bytes = ready!(
                    async_read
                        .get_mut()
                        .as_mut()
                        .poll_read(cx, &mut buf[reserved..])
                )?;

                if bytes == 0 {
                    *done = true;
                    return Poll::Ready(Ok(0));
                }

                *progress += bytes as u64;

                let frame = Frame::Data(bytes as u64);
                let header_len = frame.encode(buf).unwrap();
                if header_len < reserved {
                    buf.copy_within(reserved..reserved + bytes, header_len);
                }

                Poll::Ready(Ok(header_len + bytes))
            }
        }
    }
}
