use crate::{body::BodyType, h3::Frame};
use futures_lite::{AsyncRead, ready};
use std::{io, pin::Pin, task::Poll};

/// This is a temporary wrapper type that will eventually be integrated into Body's AsyncRead
/// through a Version switch, but for now it's easier to keep it distinct
pub(crate) struct H3BodyWrapper {
    pub(crate) body: BodyType,
    /// Whether the DATA frame header has been written for known-length bodies.
    /// Always false for unknown-length (each poll emits its own frame).
    header_written: bool,
}

impl H3BodyWrapper {
    pub(crate) fn new(body: BodyType) -> Self {
        Self {
            body,
            header_written: false,
        }
    }
}

impl AsyncRead for H3BodyWrapper {
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

                let bytes = ready!(async_read.as_mut().poll_read(cx, &mut buf[reserved..]))?;

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
