use std::{borrow::Cow, task::Poll};

use arbitrary::{Arbitrary, Unstructured};
use futures_io::AsyncRead;

const MAX_NUM_READS: usize = 20;
const MAX_READ_SIZE: usize = 100;

/// This represents one buffer of data delivered to the application from a socket.
#[derive(Debug, Clone)]
struct SocketRead(Vec<u8>);

impl<'a> Arbitrary<'a> for SocketRead {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let len = u.arbitrary_len::<u8>()?;
        let clamped_len = std::cmp::min(len, MAX_READ_SIZE);
        Ok(Self(u.bytes(clamped_len)?.to_owned()))
    }
}

/// This represents data delivered to an application from a socket, split into separate buffers.
#[derive(Debug, Clone)]
pub struct SocketReads {
    reads: Vec<SocketRead>,
}

impl<'a> Arbitrary<'a> for SocketReads {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let reads = u
            .arbitrary_iter::<SocketRead>()?
            .take(MAX_NUM_READS)
            .filter(|read| {
                if let Ok(read) = read {
                    // Skip empty buffers, because those are reserved for EOFs.
                    !read.0.is_empty()
                } else {
                    true
                }
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { reads })
    }
}

impl SocketReads {
    pub fn from_utf8_lossy(&self) -> Vec<Cow<'_, str>> {
        self.reads
            .iter()
            .map(|read| String::from_utf8_lossy(&read.0))
            .collect::<Vec<_>>()
    }
}

/// An implementation of `AsyncRead` that returns fuzzer-generated data, in multiple chunks.
pub struct FuzzTransport {
    data: SocketReads,
    buffer_index: usize,
    buffer_position: usize,
}

impl FuzzTransport {
    pub fn new(socket_reads: SocketReads) -> Self {
        Self {
            data: socket_reads,
            buffer_index: 0,
            buffer_position: 0,
        }
    }
}

impl AsyncRead for FuzzTransport {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut [u8],
    ) -> std::task::Poll<futures_io::Result<usize>> {
        if self.buffer_index >= self.data.reads.len() {
            return Poll::Ready(Ok(0));
        }
        let rest = &self.data.reads[self.buffer_index].0[self.buffer_position..];
        if rest.len() < buf.len() {
            // cannot fill buffer with current read.
            let len = rest.len();
            buf[..len].copy_from_slice(rest);
            // advance to next read for next poll.
            self.buffer_index += 1;
            self.buffer_position = 0;
            Poll::Ready(Ok(len))
        } else {
            // can fill buffer with current read.
            let len = buf.len();
            buf.copy_from_slice(&rest[..len]);
            // advance position to mark what was consumed, reuse same read in next poll.
            self.buffer_position += len;
            Poll::Ready(Ok(len))
        }
    }
}
