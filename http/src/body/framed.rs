//! Write-oriented body consumption: [`Body::write_into`] moves body content directly into a
//! [`BufWriter`]'s buffer, applying wire framing as it goes.
//!
//! This inverts the `AsyncRead`-based send path (`BufWriter::copy_from(&mut body)`), which
//! required an intermediary buffer between the body source and the sink buffer plus a
//! payload memmove per chunk to make room for framing prefixes. Here, streaming content is
//! read directly into reserved space in the sink buffer with the framing prefix backfilled
//! in place, and static content is presented to the sink as one borrowed slice — the
//! sink's vectored overflow path turns `[headers, body]` into a single writev.

use super::{Body, BodyType};
use crate::{BufWriter, Headers, HttpConfig, body::BodySource, h3::Frame};
use futures_lite::{AsyncWrite, AsyncWriteExt};
use std::{
    future::poll_fn,
    io::{Result, Write},
    pin::Pin,
    task::{Context, Poll, ready},
};

/// Wire framing applied by [`Body::write_into`] as it moves body content into a send buffer.
///
/// Framing is the caller's decision, derived from the finalized headers or protocol —
/// unlike the (deprecated-in-place) framed `AsyncRead` impl on [`Body`], which carried
/// framing decisions as internal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyFraming {
    /// Content bytes pass through unframed: fixed-length (`Content-Length`) or
    /// close-delimited h1 bodies.
    Raw,
    /// h1 chunked transfer coding; each streaming read becomes one chunk. At end of
    /// content the `0\r\n` last-chunk marker is written unless `keep_open` — the caller
    /// owns the trailer section and its terminating CRLF either way.
    Chunked {
        /// Leave the chunked stream unterminated (no `0\r\n` last-chunk marker) for a
        /// following upgrade to continue and eventually close.
        keep_open: bool,
    },
    /// h3 DATA frames: one frame spanning the whole body when the length is known up
    /// front, one frame per streaming read otherwise.
    H3Data,
}

/// Floor for `HttpConfig::body_write_chunk_len` — a reservation must at least fit a
/// framing prefix plus a byte of payload.
const MIN_CHUNK_LEN: usize = 16;

impl Body {
    /// Write this body's content into `sink`'s buffer, framed per `framing`.
    ///
    /// Streaming content is read and framed in slices of at most
    /// `config.body_write_chunk_len` bytes, the sink buffer is written through to the
    /// transport whenever it holds at least that many bytes, and the task yields to the
    /// runtime every `config.copy_loops_per_yield` reads.
    ///
    /// Returns the body's trailers, if its source produced any; only chunked and h3
    /// framing have a wire representation for them, and writing that representation
    /// (trailer section, HEADERS frame) is the caller's responsibility. So is the final
    /// flush: content past the last drain remains in `sink`'s buffer so trailers and
    /// terminators can coalesce into the same write.
    ///
    /// While the body source is pending, `sink` is flushed (buffer and inner writer) so
    /// already-produced content reaches the peer rather than stalling in the buffer.
    ///
    /// # Errors
    ///
    /// Returns any io error encountered reading the body source or writing to the sink.
    #[cfg(feature = "unstable")]
    #[doc(hidden)]
    pub async fn write_into<W: AsyncWrite + Unpin>(
        self,
        sink: &mut BufWriter<W>,
        framing: BodyFraming,
        config: &HttpConfig,
    ) -> Result<Option<Headers>> {
        write_into(self, sink, framing, config).await
    }

    /// Write this body's content into `sink`'s buffer, framed per `framing`. See the
    /// `unstable` variant for the full contract.
    #[cfg(not(feature = "unstable"))]
    pub(crate) async fn write_into<W: AsyncWrite + Unpin>(
        self,
        sink: &mut BufWriter<W>,
        framing: BodyFraming,
        config: &HttpConfig,
    ) -> Result<Option<Headers>> {
        write_into(self, sink, framing, config).await
    }
}

async fn write_into<W: AsyncWrite + Unpin>(
    body: Body,
    sink: &mut BufWriter<W>,
    framing: BodyFraming,
    config: &HttpConfig,
) -> Result<Option<Headers>> {
    match body.0 {
        BodyType::Empty => {
            if framing == (BodyFraming::Chunked { keep_open: false }) {
                sink.buffer_mut().extend_from_slice(b"0\r\n");
            }
            Ok(None)
        }

        BodyType::Static { content, cursor } => {
            write_static(&content[cursor..], sink, framing).await?;
            Ok(None)
        }

        BodyType::Streaming {
            async_read,
            progress,
            len,
            done,
            ..
        } => {
            write_streaming(
                async_read.into_inner(),
                len,
                progress,
                done,
                sink,
                framing,
                config,
            )
            .await
        }
    }
}

async fn write_static<W: AsyncWrite + Unpin>(
    content: &[u8],
    sink: &mut BufWriter<W>,
    framing: BodyFraming,
) -> Result<()> {
    match framing {
        BodyFraming::Raw => {
            if !content.is_empty() {
                sink.write_all(content).await?;
            }
        }

        BodyFraming::Chunked { keep_open } => {
            if !content.is_empty() {
                write!(sink.buffer_mut(), "{:X}\r\n", content.len())?;
                sink.write_all(content).await?;
                sink.buffer_mut().extend_from_slice(b"\r\n");
            }
            if !keep_open {
                sink.buffer_mut().extend_from_slice(b"0\r\n");
            }
        }

        BodyFraming::H3Data => {
            if !content.is_empty() {
                write_h3_frame_header(sink.buffer_mut(), content.len() as u64);
                sink.write_all(content).await?;
            }
        }
    }
    Ok(())
}

async fn write_streaming<W: AsyncWrite + Unpin>(
    mut source: Pin<Box<dyn BodySource>>,
    len: Option<u64>,
    mut progress: u64,
    mut done: bool,
    sink: &mut BufWriter<W>,
    framing: BodyFraming,
    config: &HttpConfig,
) -> Result<Option<Headers>> {
    let chunk_len = config.body_write_chunk_len.max(MIN_CHUNK_LEN);
    let loops_per_yield = config.copy_loops_per_yield;
    // A known length means one DATA frame spans the whole body; its header goes out
    // before any content and the loop below appends raw payload.
    if !done
        && framing == BodyFraming::H3Data
        && let Some(len) = len
        && len > 0
    {
        write_h3_frame_header(sink.buffer_mut(), len);
    }

    poll_fn(|cx| {
        poll_write_streaming(
            cx,
            source.as_mut(),
            len,
            &mut progress,
            &mut done,
            sink,
            framing,
            chunk_len,
            loops_per_yield,
        )
    })
    .await?;

    Ok(source.as_mut().trailers())
}

#[allow(
    clippy::too_many_arguments,
    reason = "poll fn over write_streaming's locals"
)]
fn poll_write_streaming<W: AsyncWrite + Unpin>(
    cx: &mut Context<'_>,
    mut source: Pin<&mut dyn BodySource>,
    len: Option<u64>,
    progress: &mut u64,
    done: &mut bool,
    sink: &mut BufWriter<W>,
    framing: BodyFraming,
    chunk_len: usize,
    loops_per_yield: usize,
) -> Poll<Result<()>> {
    if *done {
        return Poll::Ready(Ok(()));
    }

    for _ in 0..loops_per_yield {
        if sink.buffer_mut().len() >= chunk_len {
            ready!(Pin::new(&mut *sink).poll_flush_buf(cx))?;
        }

        let cap = len.map_or(chunk_len, |len| {
            usize::try_from(len - *progress).map_or(chunk_len, |remaining| remaining.min(chunk_len))
        });

        // Reserve prefix + payload space at the buffer tail and read content directly
        // into the payload slot. The prefix width is only known once the read size is,
        // so reserve the widest prefix `cap` could need and backfill.
        let reserved = match framing {
            BodyFraming::Raw => 0,
            BodyFraming::Chunked { .. } => hex_width(cap) + 2,
            // Known-length DATA frame header was written before the loop.
            BodyFraming::H3Data if len.is_some() => 0,
            BodyFraming::H3Data => Frame::Data(cap as u64).encoded_len(),
        };

        let buffer = sink.buffer_mut();
        let start = buffer.len();
        buffer.resize(start + reserved + cap, 0);

        match source
            .as_mut()
            .poll_read(cx, &mut buffer[start + reserved..])
        {
            Poll::Pending => {
                buffer.truncate(start);
                ready!(Pin::new(&mut *sink).poll_flush(cx))?;
                return Poll::Pending;
            }

            Poll::Ready(Err(e)) => {
                buffer.truncate(start);
                return Poll::Ready(Err(e));
            }

            Poll::Ready(Ok(0)) => {
                buffer.truncate(start);
                *done = true;
                if framing == (BodyFraming::Chunked { keep_open: false }) {
                    buffer.extend_from_slice(b"0\r\n");
                }
                return Poll::Ready(Ok(()));
            }

            Poll::Ready(Ok(bytes)) => {
                *progress += bytes as u64;

                match framing {
                    BodyFraming::Raw => buffer.truncate(start + bytes),

                    BodyFraming::H3Data if len.is_some() => buffer.truncate(start + bytes),

                    BodyFraming::Chunked { .. } => {
                        let prefix = hex_width(bytes) + 2;
                        let mut slot = &mut buffer[start..start + prefix];
                        write!(slot, "{bytes:X}\r\n")?;
                        if prefix < reserved {
                            buffer.copy_within(
                                start + reserved..start + reserved + bytes,
                                start + prefix,
                            );
                        }
                        buffer.truncate(start + prefix + bytes);
                        buffer.extend_from_slice(b"\r\n");
                    }

                    BodyFraming::H3Data => {
                        let frame = Frame::Data(bytes as u64);
                        let prefix = frame.encoded_len();
                        frame.encode(&mut buffer[start..start + prefix]);
                        if prefix < reserved {
                            buffer.copy_within(
                                start + reserved..start + reserved + bytes,
                                start + prefix,
                            );
                        }
                        buffer.truncate(start + prefix + bytes);
                    }
                }
            }
        }
    }

    cx.waker().wake_by_ref();
    Poll::Pending
}

fn write_h3_frame_header(buffer: &mut Vec<u8>, len: u64) {
    let frame = Frame::Data(len);
    let start = buffer.len();
    buffer.resize(start + frame.encoded_len(), 0);
    frame.encode(&mut buffer[start..]);
}

/// The number of hex digits in `n`'s chunk-size representation.
fn hex_width(n: usize) -> usize {
    if n == 0 { 1 } else { n.ilog(16) as usize + 1 }
}

#[cfg(test)]
mod tests;
