use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, ready},
};

/// Shuts down the sending side of a transport, then reads and discards until the peer closes its
/// side.
///
/// Yields to the runtime after every `reads_per_yield` consecutive reads, so a peer that keeps
/// sending can't stop this from returning `Pending`, which would starve a timeout racing it.
pub(crate) struct GracefulClose<Transport> {
    transport: Transport,
    scratch: Vec<u8>,
    state: State,
    reads_per_yield: usize,
    reads_since_yield: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    ShuttingDown,
    Draining,
    Done,
}

/// Smallest discard buffer, for a caller handing over an empty allocation.
const MIN_SCRATCH_LEN: usize = 512;

impl<Transport> GracefulClose<Transport> {
    /// `scratch` is an allocation to read into and discard; its contents are ignored.
    pub(crate) fn new(transport: Transport, mut scratch: Vec<u8>, reads_per_yield: usize) -> Self {
        scratch.clear();
        scratch.resize(scratch.capacity().max(MIN_SCRATCH_LEN), 0);
        Self {
            transport,
            scratch,
            state: State::ShuttingDown,
            reads_per_yield: reads_per_yield.max(1),
            reads_since_yield: 0,
        }
    }

    /// A close that completes immediately, for a peer that has already closed its side.
    pub(crate) fn finished(transport: Transport) -> Self {
        Self {
            transport,
            scratch: Vec::new(),
            state: State::Done,
            reads_per_yield: 1,
            reads_since_yield: 0,
        }
    }
}

impl<Transport> Future for GracefulClose<Transport>
where
    Transport: AsyncRead + AsyncWrite + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let this = self.get_mut();
        loop {
            match this.state {
                State::ShuttingDown => {
                    this.state = match ready!(Pin::new(&mut this.transport).poll_close(cx)) {
                        Ok(()) => State::Draining,
                        Err(_) => State::Done,
                    };
                }

                State::Draining => {
                    if this.reads_since_yield >= this.reads_per_yield {
                        this.reads_since_yield = 0;
                        cx.waker().wake_by_ref();
                        return Poll::Pending;
                    }
                    match Pin::new(&mut this.transport).poll_read(cx, &mut this.scratch) {
                        Poll::Pending => {
                            this.reads_since_yield = 0;
                            return Poll::Pending;
                        }
                        Poll::Ready(Ok(0) | Err(_)) => this.state = State::Done,
                        Poll::Ready(Ok(_)) => this.reads_since_yield += 1,
                    }
                }

                State::Done => return Poll::Ready(()),
            }
        }
    }
}
