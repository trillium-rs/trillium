use std::convert::TryInto;
use std::io::ErrorKind;
use std::iter;
use std::{
    fmt::{self, Formatter},
    pin::Pin,
    task::{Context, Poll},
};

use crate::MutCow;
use encoding_rs::Encoding;
use futures_lite::io::{self, BufReader};
use futures_lite::{ready, AsyncRead, AsyncReadExt, AsyncWrite, Stream};
use http_types::Body;
use httparse::Status;
use Poll::{Pending, Ready};
use ReceivedBodyState::{Chunked, End, FixedLength, Start};

macro_rules! trace {
    ($s:literal, $($arg:tt)+) => (
        log::trace!(concat!(":{} ", $s), line!(), $($arg)+);
    )
}

pub struct ReceivedBody<'conn, RW> {
    content_length: Option<u64>,
    buffer: MutCow<'conn, Option<Vec<u8>>>,
    rw: Option<MutCow<'conn, RW>>,
    state: MutCow<'conn, ReceivedBodyState>,
    on_completion: Option<Box<dyn Fn(RW) + Send + Sync + 'static>>,
    encoding: &'static Encoding,
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
            on_completion: self.on_completion,
            encoding: self.encoding,
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
        encoding: &'static Encoding,
    ) -> Self {
        Self {
            content_length,
            buffer: buffer.into(),
            rw: Some(rw.into()),
            state: state.into(),
            on_completion,
            encoding,
        }
    }

    pub async fn read_string(self) -> crate::Result<String> {
        let encoding = self.encoding;
        let bytes = self.read_bytes().await?;

        let (s, _, _) = encoding.decode(&bytes);
        Ok(s.to_string())
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
                trace!(
                    "have {} bytes of pending data but can only use {}",
                    len,
                    buf.len()
                );
                let remaining = buffer.split_off(buf.len());
                buf.copy_from_slice(buffer);
                *buffer = remaining;
                Ready(Ok(buf.len()))
            } else {
                trace!("have {} bytes of pending data, using all of it", len);
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

fn chunk_decode(
    remaining: usize,
    mut total: usize,
    buf: &mut [u8],
) -> io::Result<(ReceivedBodyState, usize, Option<Vec<u8>>)> {
    let mut ranges_to_keep = vec![];
    let mut chunk_start = 0;
    let mut chunk_end = remaining;
    let (request_body_state, unused) = loop {
        if chunk_end > 2 {
            let keep_end = buf.len().min(chunk_end - 2);
            ranges_to_keep.push(chunk_start..keep_end);
            total += keep_end - chunk_start;
        }

        chunk_start = chunk_end;

        if chunk_start >= buf.len() {
            break (
                Chunked {
                    remaining: (chunk_start - buf.len()),
                    total,
                },
                None,
            );
        }

        match httparse::parse_chunk_size(&buf[chunk_start..]) {
            Ok(Status::Complete((framing_bytes, chunk_size))) => {
                chunk_start += framing_bytes;
                chunk_end = 2 + chunk_start + chunk_size as usize;

                if chunk_size == 0 {
                    break (
                        End,
                        if chunk_end < buf.len() {
                            Some(buf[chunk_end..].to_vec())
                        } else {
                            None
                        },
                    );
                }
            }

            Ok(Status::Partial) => {
                break (
                    Chunked {
                        remaining: 0,
                        total,
                    },
                    if chunk_start < buf.len() {
                        Some(buf[chunk_start..].to_vec())
                    } else {
                        None
                    },
                );
            }

            Err(httparse::InvalidChunkSize) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid chunk size",
                ));
            }
        }
    };

    let mut bytes = 0;

    for range_to_keep in ranges_to_keep.drain(..) {
        let new_bytes = bytes + range_to_keep.end - range_to_keep.start;
        buf.copy_within(range_to_keep, bytes);
        bytes = new_bytes;
    }

    Ok((request_body_state, bytes, unused))
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
        trace!("polling received body with state {:?}", &*self.state);
        let (new_body_state, bytes, unused) = match *self.state {
            ReceivedBodyState::Start => (
                match self.content_length {
                    Some(0) => ReceivedBodyState::End,

                    Some(total_length) => ReceivedBodyState::FixedLength {
                        current_index: 0,
                        total_length,
                    },

                    None => ReceivedBodyState::Chunked {
                        remaining: 0,
                        total: 0,
                    },
                },
                0,
                None,
            ),

            Chunked { remaining, total } => {
                let bytes = ready!(self.read_raw(cx, buf)?);
                chunk_decode(remaining, total, &mut buf[..bytes])?
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
                let state = if bytes == 0 || current_index == total_length {
                    End
                } else {
                    FixedLength {
                        current_index,
                        total_length,
                    }
                };

                (state, bytes, None)
            }

            End => (End, 0, None),
        };

        if let Some(unused) = unused {
            if let Some(existing) = &mut *self.buffer {
                existing.extend_from_slice(&unused);
            } else {
                *self.buffer = Some(unused);
            }
        }

        *self.state = new_body_state;

        if *self.state == End {
            if self.on_completion.is_some() && self.owns_transport() {
                let rw = self.rw.take().unwrap().unwrap_owned();
                let on_completion = self.on_completion.take().unwrap();
                on_completion(rw);
            }
            Ready(Ok(bytes))
        } else if bytes == 0 {
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Ready(Ok(bytes))
        }
    }
}

impl<'rw, RW> fmt::Debug for ReceivedBody<'rw, RW> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestBody")
            .field("state", &*self.state)
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
        total: usize,
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

#[cfg(test)]
mod chunk_decode {

    use encoding_rs::UTF_8;
    use futures_lite::io::Cursor;
    use futures_lite::{AsyncRead, AsyncReadExt};

    use super::{chunk_decode, ReceivedBody, ReceivedBodyState};

    fn assert_decoded(input: (usize, &str), expected_output: (Option<usize>, &str, Option<&str>)) {
        let (remaining, input_data) = input;

        let mut buf = input_data.to_string().into_bytes();

        let (output_state, bytes, unused) = chunk_decode(remaining, 0, &mut buf).unwrap();

        assert_eq!(
            (
                match output_state {
                    ReceivedBodyState::Chunked { remaining, .. } => Some(remaining),
                    ReceivedBodyState::End => None,
                    _ => panic!("unexpected output state {:?}", output_state),
                },
                &*String::from_utf8_lossy(&buf[0..bytes]),
                unused.as_deref().map(String::from_utf8_lossy).as_deref()
            ),
            expected_output
        );
    }

    async fn read_with_buffers_of_size<R>(reader: &mut R, size: usize) -> crate::Result<String>
    where
        R: AsyncRead + Unpin,
    {
        let mut return_buffer = vec![];
        loop {
            let mut buf = vec![0; size];
            match reader.read(&mut buf).await? {
                0 => break Ok(String::from_utf8_lossy(&return_buffer).into()),
                bytes_read => return_buffer.extend_from_slice(&buf[..bytes_read]),
            }
        }
    }

    fn full_decode_with_size(
        input: &str,
        poll_size: usize,
    ) -> crate::Result<(String, ReceivedBody<'static, Cursor<&str>>)> {
        let mut rb = ReceivedBody::new(
            None,
            None,
            Cursor::new(input),
            ReceivedBodyState::Chunked {
                remaining: 0,
                total: 0,
            },
            None,
            &UTF_8,
        );

        let output = async_io::block_on(read_with_buffers_of_size(&mut rb, poll_size))?;
        Ok((output, rb))
    }

    #[test]
    fn test_full_decode() {
        env_logger::try_init().ok();

        for size in 3..50 {
            let input = "5\r\n12345\r\n1\r\na\r\n2\r\nbc\r\n3\r\ndef\r\n0\r\n";
            let (output, _) = full_decode_with_size(input, size).unwrap();
            assert_eq!(output, "12345abcdef", "size: {}", size);

            let input = "7\r\nMozilla\r\n9\r\nDeveloper\r\n7\r\nNetwork\r\n0\r\n\r\n";
            let (output, _) = full_decode_with_size(input, size).unwrap();
            assert_eq!(output, "MozillaDeveloperNetwork", "size: {}", size);
        }
    }

    #[test]
    fn test_chunk_start() {
        assert_decoded((0, "5\r\n12345\r\n"), (Some(0), "12345", None));
        assert_decoded((0, "F\r\n1"), (Some(14 + 2), "1", None));
        assert_decoded((0, "5\r\n123"), (Some(2 + 2), "123", None));
        assert_decoded((0, "1\r\nX\r\n1\r\nX\r\n"), (Some(0), "XX", None));
        assert_decoded((0, "1\r\nX\r\n1\r\nX\r\n1"), (Some(0), "XX", Some("1")));
        assert_decoded((0, "FFF\r\n"), (Some(0xfff + 2), "", None));
        assert_decoded((10, "hello"), (Some(5), "hello", None));
        assert_decoded(
            (7, "hello\r\nA\r\n world"),
            (Some(4 + 2), "hello world", None),
        );
        assert_decoded(
            (0, "e\r\ntest test test\r\n0\r\n\r\n"),
            (None, "test test test", None),
        );
        assert_decoded(
            (0, "1\r\n_\r\n0\r\n\r\nnext request"),
            (None, "_", Some("next request")),
        );
        assert_decoded((7, "hello\r\n0\r\n\r\n"), (None, "hello", None));
    }
}
