use crate::WebSocketConfig;
use futures_lite::{AsyncRead, AsyncWrite, Stream, ready};
use std::{
    io::{self, ErrorKind, Read, Write},
    pin::Pin,
    task::{Context, Poll},
};
use swansong::Interrupt;
use trillium::Transport;
use tungstenite::{
    Error, Message,
    protocol::{Role, WebSocketContext},
};

pub(crate) type MessageResult = Result<Message, Error>;

/// tungstenite zero-fills this many bytes before every pull from [`Io`], so pulls are kept small.
/// Socket reads are sized by [`WebSocketConfig::read_buffer_size`] instead, into a buffer that is
/// zeroed only once.
const PULL_LEN: usize = 1024;

const DEFAULT_READ_BUFFER_LEN: usize = 8 * 1024;

pub(crate) fn default_config() -> WebSocketConfig {
    WebSocketConfig::default().read_buffer_size(DEFAULT_READ_BUFFER_LEN)
}

/// Drives a [`WebSocketContext`] over in-memory buffers, performing all transport io itself.
pub(crate) struct Driver {
    context: WebSocketContext,
    io: Io,
    write_buffer_len: usize,
    ended: bool,
}

/// The `Read + Write` that tungstenite sees. Reads drain bytes already received from the
/// transport and report `WouldBlock` once they run out; writes append to the outbound buffer.
struct Io {
    transport: Box<dyn Transport>,
    inbound: Box<[u8]>,
    inbound_start: usize,
    inbound_end: usize,
    eof: bool,
    outbound: Vec<u8>,
    outbound_written: usize,
    needs_flush: bool,
}

impl Driver {
    pub(crate) fn new(
        transport: Box<dyn Transport>,
        partially_read: Vec<u8>,
        role: Role,
        config: WebSocketConfig,
    ) -> Self {
        let read_buffer_len = config.read_buffer_size.max(1);
        let write_buffer_len = config.write_buffer_size;
        let config = Some(config.read_buffer_size(PULL_LEN).write_buffer_size(0));

        let context = if partially_read.is_empty() {
            WebSocketContext::new(role, config)
        } else {
            WebSocketContext::from_partially_read(partially_read, role, config)
        };

        Self {
            context,
            io: Io {
                transport,
                inbound: vec![0; read_buffer_len].into_boxed_slice(),
                inbound_start: 0,
                inbound_end: 0,
                eof: false,
                outbound: Vec::new(),
                outbound_written: 0,
                needs_flush: false,
            },
            write_buffer_len,
            ended: false,
        }
    }

    pub(crate) fn write(&mut self, message: Message) -> Result<(), Error> {
        self.context.write(&mut self.io, message)
    }

    /// Queues our close frame, or the reply to the peer's. Unlike writing a [`Message::Close`],
    /// which tungstenite rejects once the peer has closed, this also completes a handshake the
    /// peer started.
    pub(crate) fn close(&mut self) -> Result<(), Error> {
        match self.context.close(&mut self.io, None) {
            Err(Error::ConnectionClosed) => Ok(()),
            other => other,
        }
    }

    /// Ready once queued output is below `write_buffer_size`, writing it out until then.
    pub(crate) fn poll_ready_to_write(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        if self.io.outbound.len() - self.io.outbound_written < self.write_buffer_len {
            Poll::Ready(Ok(()))
        } else {
            self.poll_write_out(cx)
        }
    }

    pub(crate) fn poll_write_out(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        self.io.poll_write_out(cx).map_err(Error::Io)
    }

    pub(crate) fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        let queued = self.context.flush(&mut self.io);
        ready!(self.poll_write_out(cx))?;
        Poll::Ready(match queued {
            // tungstenite reports a completed close handshake here, after queueing the last frame
            Err(Error::ConnectionClosed) => Ok(()),
            other => other,
        })
    }
}

/// Reads only. See [`poll_inbound`] for the write-out that has to accompany it.
impl Stream for Driver {
    type Item = MessageResult;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.ended {
            return Poll::Ready(None);
        }

        loop {
            match this.context.read(&mut this.io) {
                Ok(message) => return Poll::Ready(Some(Ok(message))),

                Err(Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => {
                    match ready!(this.io.poll_fill(cx)) {
                        Ok(()) => {}
                        Err(e) => {
                            this.ended = true;
                            return Poll::Ready(Some(Err(Error::Io(e))));
                        }
                    }
                }

                Err(Error::ConnectionClosed | Error::AlreadyClosed) => {
                    this.ended = true;
                    return Poll::Ready(None);
                }

                Err(e) => {
                    this.ended = true;
                    return Poll::Ready(Some(Err(e)));
                }
            }
        }
    }
}

/// Polls for the next inbound message, writing out queued output whenever there isn't one.
pub(crate) fn poll_inbound(
    driver: &mut Interrupt<Driver>,
    cx: &mut Context<'_>,
) -> Poll<Option<MessageResult>> {
    let poll = Pin::new(&mut *driver).poll_next(cx);
    write_out_unless_message(poll, || driver.poll_write_out(cx))
}

/// Queued output includes replies to pings and closes, which tungstenite produces while reading,
/// so this is what gets them onto the socket. A stream that has ended reports its end only once
/// that output is written.
pub(crate) fn write_out_unless_message(
    poll: Poll<Option<MessageResult>>,
    write_out: impl FnOnce() -> Poll<Result<(), Error>>,
) -> Poll<Option<MessageResult>> {
    match poll {
        Poll::Ready(Some(item)) => Poll::Ready(Some(item)),
        Poll::Ready(None) => write_out().map(|_| None),
        Poll::Pending => {
            // Errors resurface on the next send or flush.
            if let Poll::Ready(Err(e)) = write_out() {
                log::debug!("websocket write error: {e}");
            }
            Poll::Pending
        }
    }
}

impl Io {
    /// Only called once tungstenite has pulled everything previously received, so every read
    /// starts at the front of `inbound`.
    fn poll_fill(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let len = ready!(Pin::new(&mut self.transport).poll_read(cx, &mut self.inbound))?;
        self.inbound_start = 0;
        self.inbound_end = len;
        self.eof = len == 0;
        Poll::Ready(Ok(()))
    }

    fn poll_write_out(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.outbound_written < self.outbound.len() {
            let written = ready!(
                Pin::new(&mut self.transport)
                    .poll_write(cx, &self.outbound[self.outbound_written..])
            )?;
            if written == 0 {
                return Poll::Ready(Err(ErrorKind::WriteZero.into()));
            }
            self.outbound_written += written;
            self.needs_flush = true;
        }

        if self.outbound_written > 0 {
            self.outbound.clear();
            self.outbound_written = 0;
        }

        if self.needs_flush {
            ready!(Pin::new(&mut self.transport).poll_flush(cx))?;
            self.needs_flush = false;
        }

        Poll::Ready(Ok(()))
    }
}

impl Read for Io {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let available = &self.inbound[self.inbound_start..self.inbound_end];
        if available.is_empty() {
            return if self.eof {
                Ok(0)
            } else {
                Err(ErrorKind::WouldBlock.into())
            };
        }
        let len = available.len().min(buf.len());
        buf[..len].copy_from_slice(&available[..len]);
        self.inbound_start += len;
        Ok(len)
    }
}

impl Write for Io {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.outbound.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
