use std::convert::TryInto;
use std::io::ErrorKind;
use std::iter;
use std::ops::{Deref, DerefMut};
use std::{
    fmt::{self, Formatter},
    pin::Pin,
    task::{Context, Poll},
};

use futures_lite::io::{self, BufReader};
use futures_lite::{ready, AsyncRead, AsyncReadExt, AsyncWrite, Stream};
use http_types::Body;
use httparse::Status;
use Poll::{Pending, Ready};
use ReceivedBodyState::{Chunked, End, FixedLength, Start};

pub enum MutCow<'a, T> {
    Owned(T),
    Borrowed(&'a mut T),
}

impl<'a, T> MutCow<'a, T> {
    pub fn is_owned(&self) -> bool {
        matches!(self, MutCow::Owned(_))
    }

    pub fn is_borrowed(&self) -> bool {
        matches!(self, MutCow::Borrowed(_))
    }

    pub fn unwrap_owned(self) -> T {
        match self {
            MutCow::Owned(t) => t,
            _ => panic!("attempted to unwrap a borrow"),
        }
    }
}

impl<'a, T> Deref for MutCow<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            MutCow::Owned(t) => t,
            MutCow::Borrowed(t) => &**t,
        }
    }
}

impl<'a, T> DerefMut for MutCow<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            MutCow::Owned(t) => t,
            MutCow::Borrowed(t) => *t,
        }
    }
}

impl<T> From<T> for MutCow<'static, T> {
    fn from(t: T) -> Self {
        Self::Owned(t)
    }
}

impl<'a, T> From<&'a mut T> for MutCow<'a, T> {
    fn from(t: &'a mut T) -> Self {
        Self::Borrowed(t)
    }
}

pub struct ReceivedBody<'conn, RW> {
    content_length: Option<u64>,
    buffer: MutCow<'conn, Option<Vec<u8>>>,
    rw: Option<MutCow<'conn, RW>>,
    state: MutCow<'conn, ReceivedBodyState>,
    name: &'static str,
    on_completion: Option<Box<dyn Fn(RW) + Send + Sync + 'static>>,
}

impl<'conn, RW> ReceivedBody<'conn, RW>
where
    RW: AsyncRead + Unpin + Send + Sync + Clone + 'static,
{
    pub fn into_owned_by_cloning_transport(mut self) -> ReceivedBody<'static, RW> {
        ReceivedBody {
            content_length: self.content_length,
            buffer: MutCow::Owned(self.buffer.take()),
            rw: self.rw.map(|rw| MutCow::Owned((*rw).clone())),
            state: MutCow::Owned(*self.state),
            name: self.name,
            on_completion: self.on_completion,
        }
    }
}

impl<'conn, RW> ReceivedBody<'conn, RW>
where
    RW: AsyncRead + Unpin + Send + Sync + 'static,
{
    pub fn new(
        content_length: Option<u64>,
        buffer: impl Into<MutCow<'conn, Option<Vec<u8>>>>,
        rw: impl Into<MutCow<'conn, RW>>,
        state: impl Into<MutCow<'conn, ReceivedBodyState>>,
        on_completion: Option<Box<dyn Fn(RW) + Send + Sync + 'static>>,
        name: &'static str,
    ) -> Self {
        Self {
            content_length,
            buffer: buffer.into(),
            rw: Some(rw.into()),
            state: state.into(),
            on_completion,
            name,
        }
    }

    pub async fn read_string(mut self) -> crate::Result<String> {
        let mut string = if let Some(len) = self.content_length {
            String::with_capacity(len.try_into().unwrap_or_else(|_| usize::max_value()))
        } else {
            String::new()
        };

        self.read_to_string(&mut string).await?;
        Ok(string)
    }

    fn owns_transport(&self) -> bool {
        self.rw.as_ref().map(|rw| rw.is_owned()).unwrap_or_default()
    }

    pub async fn read_bytes(mut self) -> crate::Result<Vec<u8>> {
        let mut vec = if let Some(len) = self.content_length {
            Vec::with_capacity(len.try_into().unwrap_or_else(|_| usize::max_value()))
        } else {
            Vec::new()
        };

        self.read_to_end(&mut vec).await?;
        Ok(vec)
    }

    fn read_raw(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        if let Some(rw) = self.rw.as_mut() {
            read_raw(&mut *self.buffer, &mut **rw, cx, buf)
        } else {
            Ready(Err(ErrorKind::NotConnected.into()))
        }
    }

    pub async fn drain(self) -> io::Result<u64> {
        io::copy(self, io::sink()).await
    }
}

pub fn read_raw<RW>(
    opt_buffer: &mut Option<Vec<u8>>,
    rw: &mut RW,
    cx: &mut Context<'_>,
    buf: &mut [u8],
) -> Poll<io::Result<usize>>
where
    RW: AsyncRead + Unpin + Send + Sync + 'static,
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
                Ready(Ok(buf.len()))
            } else {
                log::trace!("have {} bytes of pending data, using all of it", len);
                buf[..len].copy_from_slice(&buffer);
                *opt_buffer = None;
                match Pin::new(rw).poll_read(cx, &mut buf[len..]) {
                    Ready(Ok(e)) => Ready(Ok(e + len)),
                    Pending => Ready(Ok(len)),
                    other => other,
                }
            }
        }

        None => Pin::new(rw).poll_read(cx, buf),
    }
}

fn chunk_decode(remaining: usize, buf: &mut [u8]) -> (ReceivedBodyState, usize, Option<Vec<u8>>) {
    let mut ranges_to_keep = vec![];
    let mut chunk_start = 0;
    let mut chunk_end = remaining;
    let (request_body_state, unused) = loop {
        if chunk_end >= buf.len() {
            ranges_to_keep.push(chunk_start..buf.len());
            break (
                Chunked {
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
                Chunked {
                    remaining: (chunk_start - buf.len()),
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
                        End,
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
                    Chunked { remaining: 0 },
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

    use super::{chunk_decode, utf8, ReceivedBodyState};

    fn assert_decoded(input: (usize, &str), expected_output: (Option<usize>, &str, Option<&str>)) {
        let (remaining, input_data) = input;

        let mut buf = input_data.to_string().into_bytes();

        let (output_state, bytes, unused) = chunk_decode(remaining, &mut buf);

        assert_eq!(
            (
                match output_state {
                    ReceivedBodyState::Chunked { remaining } => Some(remaining),
                    ReceivedBodyState::End => None,
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
impl<'conn, RW> Stream for ReceivedBody<'conn, RW>
where
    RW: AsyncRead + Unpin + Send + Sync + 'static,
{
    type Item = Vec<u8>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut bytes = 0;
        let mut vec = vec![0; STREAM_READ_BUF_LENGTH];

        loop {
            match Pin::new(&mut *self).poll_read(cx, &mut vec[bytes..]) {
                Pending if bytes == 0 => return Pending,
                Ready(Ok(0)) if bytes == 0 => return Ready(None),
                Pending | Ready(Ok(0)) => {
                    vec.truncate(bytes);
                    return Ready(Some(vec));
                }
                Ready(Ok(new_bytes)) => {
                    bytes += new_bytes;
                    vec.extend(iter::repeat(0).take(bytes + STREAM_READ_BUF_LENGTH - vec.len()));
                }
                _ => panic!(),
            }
        }
    }
}

impl<'conn, RW> AsyncRead for ReceivedBody<'conn, RW>
where
    RW: AsyncRead + Unpin + Send + Sync + 'static,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let (new_body_state, bytes) = match *self.state {
            ReceivedBodyState::Start => (End, 0),

            Chunked { remaining } => {
                let bytes = ready!(self.read_raw(cx, buf)?);
                let (new_state, bytes, unused_data) = chunk_decode(remaining, &mut buf[..bytes]);
                *self.buffer = unused_data;
                (new_state, bytes)
            }

            FixedLength {
                current_index,
                total_length,
            } => {
                let len = buf.len();
                let remaining = (total_length - current_index) as usize;
                let buf = &mut buf[..len.min(remaining)];
                let bytes = ready!(self.read_raw(cx, buf)?);
                let current_index = current_index + bytes as u64;
                if bytes == 0 || current_index == total_length {
                    (End, bytes)
                } else {
                    (
                        FixedLength {
                            current_index,
                            total_length,
                        },
                        bytes,
                    )
                }
            }

            End => (End, 0),
        };

        *self.state = new_body_state;

        if *self.state == End && self.on_completion.is_some() && self.owns_transport() {
            let rw = self.rw.take().unwrap().unwrap_owned();
            let on_completion = self.on_completion.take().unwrap();
            on_completion(rw);
        }

        Ready(Ok(bytes))
    }
}

impl<'rw, RW> fmt::Debug for ReceivedBody<'rw, RW> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestBody")
            .field("state", &*self.state)
            .field("name", &self.name)
            .field("content_length", &self.content_length)
            .field(
                "buffer",
                &self.buffer.as_deref().map(String::from_utf8_lossy),
            )
            .field("on_completion", &self.on_completion.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ReceivedBodyState {
    Start,
    Chunked {
        remaining: usize,
    },

    FixedLength {
        current_index: u64,
        total_length: u64,
    },
    End,
}

impl Default for ReceivedBodyState {
    fn default() -> Self {
        Start
    }
}

impl<RW> From<ReceivedBody<'static, RW>> for Body
where
    RW: AsyncRead + AsyncWrite + Send + Sync + Unpin + 'static,
{
    fn from(rb: ReceivedBody<'static, RW>) -> Self {
        let len = rb.content_length.map(|cl| cl as u64);
        Body::from_reader(BufReader::new(rb), len)
    }
}
