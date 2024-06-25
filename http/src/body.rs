use futures_lite::{io::Cursor, ready, AsyncRead, AsyncReadExt};
use std::{
    borrow::Cow,
    fmt::Debug,
    io::{Error, ErrorKind, Result},
    pin::Pin,
    task::{Context, Poll},
};
use BodyType::{Empty, Static, Streaming};

/// The trillium representation of a http body. This can contain
/// either `&'static [u8]` content, `Vec<u8>` content, or a boxed
/// `AsyncRead` type.
#[derive(Debug, Default)]
pub struct Body(BodyType);

impl Body {
    /// Construct a new body from a streaming [`AsyncRead`] source. If
    /// you have the body content in memory already, prefer
    /// [`Body::new_static`] or one of the From conversions.
    pub fn new_streaming(
        async_read: impl AsyncRead + Send + Sync + 'static,
        len: Option<u64>,
    ) -> Self {
        Self(Streaming {
            async_read: Box::pin(async_read),
            len,
            done: false,
            progress: 0,
        })
    }

    /// Construct a fixed-length Body from a `Vec<u8>` or `&'static
    /// [u8]`.
    pub fn new_static(content: impl Into<Cow<'static, [u8]>>) -> Self {
        Self(Static {
            content: content.into(),
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

    /// Transform this Body into a dyn `AsyncRead`. This will wrap
    /// static content in a [`Cursor`]. Note that this is different
    /// from reading directly from the Body, which includes chunked
    /// encoding.
    pub fn into_reader(self) -> Pin<Box<dyn AsyncRead + Send + Sync>> {
        match self.0 {
            Streaming { async_read, .. } => async_read,
            Static { content, .. } => Box::pin(Cursor::new(content)),
            Empty => Box::pin(Cursor::new("")),
        }
    }

    /// Consume this body and return the full content. If the body was
    /// constructed with `[Body::new_streaming`], this will read the
    /// entire streaming body into memory, awaiting the streaming
    /// source's completion. This function will return an error if a
    /// streaming body has already been read to completion.
    ///
    /// # Errors
    ///
    /// This returns an error variant if either of the following conditions are met:
    ///
    /// there is an io error when reading from the underlying transport such as a disconnect
    /// the body has already been read to completion
    #[allow(clippy::missing_errors_doc)] // false positive
    pub async fn into_bytes(self) -> Result<Cow<'static, [u8]>> {
        match self.0 {
            Static { content, .. } => Ok(content),

            Streaming {
                mut async_read,
                len,
                progress: 0,
                done: false,
            } => {
                let mut buf = len
                    .and_then(|c| c.try_into().ok())
                    .map(Vec::with_capacity)
                    .unwrap_or_default();

                async_read.read_to_end(&mut buf).await?;

                Ok(Cow::Owned(buf))
            }

            Empty => Ok(Cow::Borrowed(b"")),

            Streaming { .. } => Err(Error::new(
                ErrorKind::Other,
                "body already read to completion",
            )),
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
}

#[allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss
)]
fn max_bytes_to_read(buf_len: usize) -> usize {
    assert!(
        buf_len >= 6,
        "buffers of length {buf_len} are too small for this implementation.
            if this is a problem for you, please open an issue"
    );

    // #[allow(clippy::cast_precision_loss)] applied to the function
    // is for this line. We do not expect our buffers to be on the
    // order of petabytes, so we will not fall outside of the range of
    // integers that can be represented by f64
    let bytes_remaining_after_two_cr_lns = (buf_len - 4) as f64;

    // #[allow(clippy::cast_sign_loss)] applied to the function is for
    // this line. This is ok because we know buf_len is already a
    // usize and we are just converting it to an f64 in order to do
    // float log2(x)/4
    //
    // the maximum number of bytes that the hex representation of remaining bytes might take
    let max_bytes_of_hex_framing = (bytes_remaining_after_two_cr_lns).log2() / 4f64;

    // #[allow(clippy::cast_sign_loss)] applied to the function is for
    // this line.  This is ok because max_bytes_of_hex_framing will
    // always be smaller than bytes_remaining_after_two_cr_lns, and so
    // there is no risk of sign loss
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
            } => {
                if *done {
                    return Poll::Ready(Ok(0));
                }

                let max_bytes_to_read = max_bytes_to_read(buf.len());

                let bytes = ready!(
                    async_read
                        .as_mut()
                        .poll_read(cx, &mut buf[..max_bytes_to_read])
                )?;

                if bytes == 0 {
                    *done = true;
                } else {
                    *progress += bytes as u64;
                }

                let start = format!("{bytes:X}\r\n");
                let start_length = start.as_bytes().len();
                let total = bytes + start_length + 2;
                buf.copy_within(..bytes, start_length);
                buf[..start_length].copy_from_slice(start.as_bytes());
                buf[total - 2..total].copy_from_slice(b"\r\n");
                Poll::Ready(Ok(total))
            }
        }
    }
}

#[derive(Default)]
enum BodyType {
    #[default]
    Empty,

    Static {
        content: Cow<'static, [u8]>,
        cursor: usize,
    },

    Streaming {
        async_read: Pin<Box<dyn AsyncRead + Send + Sync + 'static>>,
        progress: u64,
        len: Option<u64>,
        done: bool,
    },
}

impl Debug for BodyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
                .field("async_read", &"..")
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
