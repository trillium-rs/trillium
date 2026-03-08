use crate::{
    Buffer,
    h3::{
        H3RequestError,
        frame::{Frame, FrameDecodeError},
    },
};
use futures_lite::{AsyncRead, AsyncReadExt, io as async_io};
use std::io;

/// A borrowed view over an `AsyncRead` transport that yields H3 frames.
///
/// Unknown/GREASE frames are automatically skipped by [`next`](Self::next).
pub(crate) struct FrameStream<'a, R> {
    reader: &'a mut R,
    buf: &'a mut Buffer,
    pending_skip: u64,
}

impl<'a, R: AsyncRead + Unpin> FrameStream<'a, R> {
    pub fn new(reader: &'a mut R, buf: &'a mut Buffer) -> Self {
        Self {
            reader,
            buf,
            pending_skip: 0,
        }
    }

    /// Yield the next meaningful frame, skipping unknown/GREASE frames.
    ///
    /// Any unconsumed payload from the previous frame is automatically
    /// drained before decoding the next frame header.
    ///
    /// Returns `Ok(None)` on clean stream end (FIN before any frame header).
    pub async fn next(&mut self) -> Result<Option<ActiveFrame<'_, 'a, R>>, H3RequestError> {
        if self.pending_skip > 0 {
            let skip = self.pending_skip as usize;
            self.pending_skip = 0;
            self.skip_bytes(skip).await?;
        }

        loop {
            match Frame::decode(self.buf) {
                Ok((Frame::Unknown(len), consumed)) => {
                    log::trace!("skipping unknown frame, payload length {len}");
                    let skip = consumed + len as usize;
                    self.skip_bytes(skip).await?;
                    continue;
                }
                Ok((frame, consumed)) => {
                    self.buf.ignore_front(consumed);
                    let remaining = match &frame {
                        Frame::Data(len) | Frame::Headers(len) => *len,
                        Frame::PushPromise {
                            field_section_length,
                            ..
                        } => *field_section_length,
                        _ => 0, // control frames are fully parsed by decode
                    };
                    return Ok(Some(ActiveFrame {
                        stream: self,
                        frame,
                        remaining,
                    }));
                }
                Err(FrameDecodeError::Incomplete) => {}
                Err(FrameDecodeError::Error(code)) => return Err(code.into()),
            }

            if !self.read_more().await? {
                return Ok(None);
            }
        }
    }

    /// Read more bytes from the transport into the buffer.
    /// Returns `false` on EOF.
    async fn read_more(&mut self) -> io::Result<bool> {
        let before = self.buf.len();
        self.buf.expand();
        let n = self.reader.read(&mut self.buf[before..]).await?;
        self.buf.truncate(before + n);
        Ok(n > 0)
    }

    /// Skip `n` bytes from the buffer, reading more from the transport if needed.
    async fn skip_bytes(&mut self, n: usize) -> io::Result<()> {
        let from_buf = n.min(self.buf.len());
        self.buf.ignore_front(from_buf);
        let remaining = (n - from_buf) as u64;

        if remaining > 0 {
            let copied =
                async_io::copy((&mut self.reader).take(remaining), async_io::sink()).await?;
            if copied < remaining {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "stream ended mid-frame payload",
                ));
            }
        }

        Ok(())
    }
}

/// A single H3 frame whose payload has not yet been consumed.
///
/// While this exists, it holds a mutable borrow on the parent [`FrameStream`],
/// preventing further frame decoding until this frame's payload is dealt with.
///
/// On drop, any unconsumed payload is recorded on the parent `FrameStream`
/// and will be skipped at the start of the next [`FrameStream::next`] call.
pub(crate) struct ActiveFrame<'b, 'a, R> {
    stream: &'b mut FrameStream<'a, R>,
    frame: Frame,
    remaining: u64,
}

impl<R: AsyncRead + Unpin> ActiveFrame<'_, '_, R> {
    /// The decoded frame header.
    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Buffer the complete payload into the stream's buffer and return it.
    ///
    /// The payload bytes remain in the buffer and are skipped when this
    /// `ActiveFrame` is dropped (or when [`FrameStream::next`] is called).
    pub async fn buffer_payload(&mut self) -> io::Result<&[u8]> {
        let len = self.remaining as usize;
        while self.stream.buf.len() < len {
            if !self.stream.read_more().await? {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "stream ended mid-frame payload",
                ));
            }
        }
        // remaining stays as-is — Drop will set pending_skip to skip these bytes
        Ok(&self.stream.buf[..len])
    }
}

impl<R> Drop for ActiveFrame<'_, '_, R> {
    fn drop(&mut self) {
        // Skip whatever portion of the payload is already in the buffer synchronously.
        let in_buf = self.stream.buf.len().min(self.remaining as usize);
        self.stream.buf.ignore_front(in_buf);
        // Any remainder beyond what's buffered gets deferred to the next next() call.
        self.stream.pending_skip = self.remaining - in_buf as u64;
    }
}
