use std::iter;
use std::{
    fmt::{self, Formatter},
    io,
    pin::Pin,
    task::{Context, Poll},
};

use futures_lite::{ready, AsyncRead, AsyncReadExt, AsyncWrite, Stream};
use httparse::Status;

use crate::Conn;

pub struct RequestBody<'conn, RW> {
    conn: &'conn mut Conn<RW>,
}

impl<'conn, RW> RequestBody<'conn, RW>
where
    RW: AsyncWrite + AsyncRead + Unpin + Send + Sync + 'static,
{
    pub fn new(conn: &'conn mut Conn<RW>) -> Self {
        Self { conn }
    }

    pub async fn read_string(mut self) -> crate::Result<String> {
        let mut string = if let Some(len) = self.conn.request_content_length()? {
            String::with_capacity(len)
        } else {
            String::new()
        };

        self.read_to_string(&mut string).await?;
        Ok(string)
    }

    pub async fn read_bytes(mut self) -> crate::Result<Vec<u8>> {
        let mut vec = if let Some(len) = self.conn.request_content_length()? {
            Vec::with_capacity(len)
        } else {
            Vec::new()
        };

        self.read_to_end(&mut vec).await?;
        Ok(vec)
    }
}

fn read_raw<RW>(
    opt_buffer: &mut Option<Vec<u8>>,
    rw: &mut RW,
    cx: &mut Context<'_>,
    buf: &mut [u8],
) -> Poll<io::Result<usize>>
where
    RW: AsyncWrite + AsyncRead + Unpin + Send + Sync + 'static,
{
    match opt_buffer {
        Some(buffer) => {
            let len = buffer.len();
            if len > buf.len() {
                log::trace!(
                    "have {} bytes of pending data but can only use {}",
                    len,
                    buf.len()
                );
                let remaining = buffer.split_off(buf.len());
                buf.copy_from_slice(buffer);
                *buffer = remaining;
                Poll::Ready(Ok(buf.len()))
            } else {
                log::trace!("have {} bytes of pending data, using all of it", len);
                &buf[..len].copy_from_slice(&buffer);
                *opt_buffer = None;
                match Pin::new(rw).poll_read(cx, &mut buf[len..]) {
                    Poll::Ready(Ok(e)) => Poll::Ready(Ok(e + len)),
                    Poll::Pending => Poll::Ready(Ok(len)),
                    other => other,
                }
            }
        }

        None => Pin::new(rw).poll_read(cx, buf),
    }
}

fn chunk_decode(remaining: usize, buf: &mut [u8]) -> (RequestBodyState, usize, Option<Vec<u8>>) {
    let mut ranges_to_keep = vec![];
    let mut chunk_start = 0;
    let mut chunk_end = remaining;
    let (request_body_state, unused) = loop {
        if chunk_end >= buf.len() {
            ranges_to_keep.push(chunk_start..buf.len());
            break (
                RequestBodyState::Chunked {
                    remaining: chunk_end - buf.len(),
                },
                None,
            );
        }

        if chunk_end > 0 {
            ranges_to_keep.push(chunk_start..chunk_end);
            chunk_start = chunk_end + 2;
        }

        if chunk_start > buf.len() {
            break (
                RequestBodyState::Chunked {
                    remaining: chunk_start - buf.len(),
                },
                None,
            );
        }

        match httparse::parse_chunk_size(&buf[chunk_start..]) {
            Ok(Status::Complete((framing_bytes, chunk_size))) => {
                log::trace!(
                    "chunk size: {:?} {:?} (\"...{}|>{}<|{}...\")",
                    framing_bytes,
                    chunk_size,
                    utf8nl(&buf[10.max(chunk_start) - 10..chunk_start]),
                    utf8nl(&buf[chunk_start..chunk_start + framing_bytes]),
                    utf8nl(
                        &buf[chunk_start + framing_bytes
                            ..buf.len().min(chunk_start + framing_bytes + 10)]
                    )
                );

                chunk_start += framing_bytes;
                chunk_end = chunk_start + chunk_size as usize;

                if chunk_size == 0 {
                    break (
                        RequestBodyState::End,
                        if chunk_end + 2 < buf.len() {
                            Some(buf[chunk_end + 2..].to_vec())
                        } else {
                            None
                        },
                    );
                }
            }

            Ok(Status::Partial) => {
                break (
                    RequestBodyState::Chunked { remaining: 0 },
                    if chunk_start < buf.len() {
                        Some(buf[chunk_start..].to_vec())
                    } else {
                        None
                    },
                );
            }

            Err(_) => {
                panic!(
                    "need to think through error handling, {:?}",
                    utf8(&buf[chunk_start..])
                )
            }
        }
    };

    let mut bytes = 0;

    for range_to_keep in ranges_to_keep.drain(..) {
        let new_bytes = bytes + range_to_keep.end - range_to_keep.start;
        buf.copy_within(range_to_keep, bytes);
        bytes = new_bytes;
    }

    (request_body_state, bytes, unused)
}

fn utf8(d: &[u8]) -> &str {
    std::str::from_utf8(d).unwrap_or("not utf8")
}

fn utf8nl(d: &[u8]) -> String {
    utf8(d).replace("\r", "\\r").replace("\n", "\\n")
}

#[cfg(test)]
mod chunk_decode {

    use super::{chunk_decode, utf8, RequestBodyState};

    fn assert_decoded(input: (usize, &str), expected_output: (Option<usize>, &str, Option<&str>)) {
        let (remaining, input_data) = input;

        let mut buf = input_data.to_string().into_bytes();

        let (output_state, bytes, unused) = chunk_decode(remaining, &mut buf);

        assert_eq!(
            (
                match output_state {
                    RequestBodyState::Chunked { remaining } => Some(remaining),
                    RequestBodyState::End => None,
                    _ => panic!("unexpected output state {:?}", output_state),
                },
                utf8(&buf[0..bytes]),
                unused.as_deref().map(utf8)
            ),
            expected_output
        );
    }

    #[test]
    fn test_chunk_start() {
        env_logger::init();
        assert_decoded((0, "5\r\n12345\r\n"), (Some(0), "12345", None));
        assert_decoded((0, "F\r\n1"), (Some(14), "1", None));
        assert_decoded((0, "5\r\n123"), (Some(2), "123", None));
        assert_decoded((0, "1\r\nX\r\n1\r\nX\r\n"), (Some(0), "XX", None));
        assert_decoded((0, "1\r\nX\r\n1\r\nX\r\n1"), (Some(0), "XX", Some("1")));
        assert_decoded((0, "FFF\r\n"), (Some(0xfff), "", None));
        assert_decoded((10, "hello"), (Some(5), "hello", None));
        assert_decoded((5, "hello\r\nA\r\n world"), (Some(4), "hello world", None));
        assert_decoded(
            (0, "e\r\ntest test test\r\n0\r\n\r\n"),
            (None, "test test test", None),
        );
        assert_decoded(
            (0, "1\r\n_\r\n0\r\n\r\nnext request"),
            (None, "_", Some("next request")),
        );
        assert_decoded((5, "hello\r\n0\r\n"), (None, "hello", None));
    }
}

const STREAM_READ_BUF_LENGTH: usize = 128;
impl<'conn, RW> Stream for RequestBody<'conn, RW>
where
    RW: AsyncWrite + AsyncRead + Unpin + Send + Sync + 'static,
{
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut bytes = 0;
        let mut vec = vec![0; STREAM_READ_BUF_LENGTH];

        loop {
            match Pin::new(&mut *self).poll_read(cx, &mut vec[bytes..]) {
                Poll::Pending if bytes == 0 => return Poll::Pending,
                Poll::Ready(Ok(0)) if bytes == 0 => return Poll::Ready(None),
                Poll::Pending | Poll::Ready(Ok(0)) => {
                    vec.truncate(bytes);
                    return Poll::Ready(Some(vec));
                }
                Poll::Ready(Ok(new_bytes)) => {
                    bytes += new_bytes;
                    vec.extend(iter::repeat(0).take(bytes + STREAM_READ_BUF_LENGTH - vec.len()));
                }
                _ => panic!(),
            }
        }
    }
}

impl<'conn, RW> AsyncRead for RequestBody<'conn, RW>
where
    RW: AsyncWrite + AsyncRead + Unpin + Send + Sync + 'static,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let Conn {
            buffer,
            rw,
            request_body_state,
            ..
        } = &mut self.conn;

        let (new_body_state, bytes) = match request_body_state {
            RequestBodyState::Start => panic!("this shouldn't happen"),

            RequestBodyState::Chunked { remaining } => {
                let bytes = ready!(read_raw(buffer, rw, cx, buf)?);
                let (new_state, bytes, unused_data) = chunk_decode(*remaining, &mut buf[..bytes]);
                *buffer = unused_data;
                (new_state, bytes)
            }

            RequestBodyState::FixedLength {
                current_index,
                total_length,
            } => {
                let len = buf.len();
                let buf = &mut buf[..len.min(*total_length - *current_index)];
                let bytes = ready!(read_raw(buffer, rw, cx, buf)?);
                if bytes == 0 {
                    (RequestBodyState::End, 0)
                } else {
                    (
                        RequestBodyState::FixedLength {
                            current_index: *current_index + bytes,
                            total_length: *total_length,
                        },
                        bytes,
                    )
                }
            }

            RequestBodyState::End => (RequestBodyState::End, 0),
        };

        *request_body_state = new_body_state;
        Poll::Ready(Ok(bytes))
    }
}

impl<'rw, RW> fmt::Debug for RequestBody<'rw, RW> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestBody").finish()
    }
}

#[derive(Debug)]
pub enum RequestBodyState {
    Start,
    Chunked {
        remaining: usize,
    },

    FixedLength {
        current_index: usize,
        total_length: usize,
    },
    End,
}
