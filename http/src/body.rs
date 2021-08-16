use futures_lite::{ready, AsyncRead, AsyncReadExt};
use std::{
    borrow::Cow,
    convert::TryInto,
    fmt::Debug,
    io::{Error, ErrorKind, Result},
    pin::Pin,
    task::{Context, Poll},
};

/// The trillium representation of a http body. This can contain
/// either `&'static [u8]` content, `Vec<u8>` content, or a boxed
/// `AsyncRead` type.
#[derive(Debug, Default)]
pub struct Body(BodyType);

impl Body {
    /// Construct a new body from a streaming (AsyncRead) source. If
    /// you have the body content in memory already, prefer
    /// [`Body::new_static`] or one of the From conversions.
    pub fn new_streaming(
        async_read: impl AsyncRead + Send + Sync + 'static,
        len: Option<u64>,
    ) -> Self {
        Self(BodyType::Streaming {
            async_read: Box::pin(async_read),
            len,
            done: false,
        })
    }

    /// Construct a fixed-length Body from a `Vec<u8>` or `&'static
    /// [u8]`.
    pub fn new_static(content: impl Into<Cow<'static, [u8]>>) -> Self {
        Self(BodyType::Static {
            content: content.into(),
            cursor: 0,
        })
    }

    /// Retrieve a borrow of the static content in this body. If this
    /// body is a streaming body, this will return None.
    pub fn static_bytes(&self) -> Option<&[u8]> {
        match &self.0 {
            BodyType::Static { content, .. } => Some(content.as_ref()),
            _ => None,
        }
    }

    /// Consume this body and return the full body. If the body was
    /// constructed with `[Body::new_streaming`], this will read the
    /// entire streaming body into memory, awaiting the streaming
    /// source's completion. This function will return an error if a
    /// streaming body has already been read to completion.
    pub async fn into_bytes(self) -> Result<Cow<'static, [u8]>> {
        match self.0 {
            BodyType::Static { content, .. } => Ok(content),

            BodyType::Streaming {
                mut async_read,
                len,
                done: false,
            } => {
                let mut buf = len
                    .and_then(|c| c.try_into().ok())
                    .map(Vec::with_capacity)
                    .unwrap_or_default();

                async_read.read_to_end(&mut buf).await?;

                Ok(Cow::Owned(buf))
            }

            BodyType::Empty => Ok(Cow::Borrowed(b"")),

            _ => Err(Error::new(
                ErrorKind::Other,
                "body already read to completion",
            )),
        }
    }

    /// returns the content length of this body, if known and
    /// available.
    pub fn len(&self) -> Option<u64> {
        self.0.len()
    }

    /// determine if the this body represents no data
    pub fn is_empty(&self) -> bool {
        match self.0 {
            BodyType::Empty => true,
            BodyType::Static { content, .. } => content.is_empty(),
            BodyType::Streaming { len, .. } => len == Some(0),
        }
    }
}

fn max_bytes_to_read(buf_len: usize) -> usize {
    if buf_len < 6 {
        // the minimum read size is of 6 represents one byte of
        // content from the body. the other five bytes are 1\r\n_\r\n
        // where _ is the actual content in question
        panic!("buffers of length {} are too small for this implementation. if this is a problem for you, please open an issue", buf_len);
    }

    let bytes_remaining_after_two_cr_lns = (buf_len - 4) as f64;

    // the maximum number of bytes that the hex representation of remaining bytes might take
    let max_bytes_of_hex_framing = bytes_remaining_after_two_cr_lns.log2() / 4f64;

    (bytes_remaining_after_two_cr_lns - max_bytes_of_hex_framing.ceil()) as usize
}

impl AsyncRead for Body {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        match &mut self.0 {
            BodyType::Empty => Poll::Ready(Ok(0)),
            BodyType::Static { content, cursor } => {
                let length = content.len();
                if length == *cursor {
                    return Poll::Ready(Ok(0));
                }
                let bytes = (length - *cursor).min(buf.len()) as usize;
                buf[0..bytes].copy_from_slice(&content[*cursor..bytes]);
                *cursor += bytes;
                Poll::Ready(Ok(bytes))
            }

            BodyType::Streaming {
                async_read,
                len: _,
                done,
            } => {
                if *done {
                    return Poll::Ready(Ok(0));
                }

                let max_bytes_to_read = max_bytes_to_read(buf.len());

                let bytes = ready!(async_read
                    .as_mut()
                    .poll_read(cx, &mut buf[..max_bytes_to_read]))?;

                if bytes == 0 {
                    *done = true;
                }

                let start = format!("{:X}\r\n", bytes);
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

enum BodyType {
    Empty,

    Static {
        content: Cow<'static, [u8]>,
        cursor: usize,
    },

    Streaming {
        async_read: Pin<Box<dyn AsyncRead + Send + Sync + 'static>>,
        len: Option<u64>,
        done: bool,
    },
}

impl Debug for BodyType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BodyType::Empty => f.debug_tuple("BodyType::Empty").finish(),
            BodyType::Static { content, cursor } => f
                .debug_struct("BodyType::Static")
                .field("content", &String::from_utf8_lossy(&*content))
                .field("cursor", cursor)
                .finish(),
            BodyType::Streaming { len, done, .. } => f
                .debug_struct("BodyType::Streaming")
                .field("async_read", &"..")
                .field("len", &len)
                .field("done", &done)
                .finish(),
        }
    }
}

impl Default for BodyType {
    fn default() -> Self {
        BodyType::Empty
    }
}

impl BodyType {
    fn len(&self) -> Option<u64> {
        match *self {
            Self::Empty => Some(0),
            Self::Static { ref content, .. } => Some(content.len() as u64),
            Self::Streaming { len, .. } => len,
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
    fn from(s: &'static [u8]) -> Self {
        Self::new_static(s)
    }
}

impl From<Vec<u8>> for Body {
    fn from(s: Vec<u8>) -> Self {
        Self::new_static(s)
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
                "\n\nexpected max_bytes_to_read({}) to be {}, but it was {}",
                input, expected, actual
            );

            // testing the test:
            let used_bytes = expected + 4 + format!("{:X}", expected).len();
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
