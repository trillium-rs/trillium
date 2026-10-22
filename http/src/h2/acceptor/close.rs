use super::{H2Driver, constants::READ_WINDOW};
use futures_lite::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Close the connection: shut down our sending side of the transport, then read and discard
    /// whatever the peer still sends until it closes its side.
    ///
    /// If the transport is dropped while the peer is still sending, the operating system answers
    /// the peer's data with a TCP reset, which can destroy data the peer hasn't read yet — our
    /// final GOAWAY, or the end of a response. Waiting for the peer to close avoids that. A peer
    /// that never closes keeps this waiting indefinitely, so bound it with a timeout.
    ///
    /// Call this once [`Self::next`] returns `None` or an error. Closing a driver that hasn't
    /// finished ends the connection without a GOAWAY, resetting any streams still in flight.
    pub async fn close(mut self) {
        if !self.finished {
            self.finish_with_current_outcome();
        }

        if self.transport.close().await.is_err() {
            return;
        }

        self.read_buf.resize(READ_WINDOW, 0);
        while let Ok(1..) = self.transport.read(&mut self.read_buf).await {}
    }
}
