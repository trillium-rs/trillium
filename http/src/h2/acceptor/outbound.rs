//! Outbound bytes plumbing for the driver.
//!
//! Two halves:
//! - [`H2Driver::poll_flush_outbound`] drains `write_buf` to the transport and issues a subsequent
//!   flush when bytes were written.
//! - The `queue_*` helpers append encoded frames to `write_buf` and set `write_flush_pending` so
//!   the next `poll_flush_outbound` issues a flush.
//!
//! All methods are on [`super::H2Driver`].

use super::{ClosedReason, H2Driver};
use crate::h2::{H2ErrorCode, H2Settings, frame};
use futures_lite::io::{AsyncRead, AsyncWrite};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll, ready},
};

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Drain `write_buf[write_cursor..]` to the transport, then flush if bytes were
    /// written. Returns `Ready(Ok(()))` when both the buffer is empty AND any pending
    /// flush has completed.
    pub(super) fn poll_flush_outbound(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.write_cursor < self.write_buf.len() {
            let n = ready!(
                Pin::new(&mut self.transport).poll_write(cx, &self.write_buf[self.write_cursor..])
            )?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::from(io::ErrorKind::WriteZero)));
            }
            self.write_cursor += n;
        }
        // Fully drained — reset the buffer so future writes start at offset 0.
        self.write_buf.clear();
        self.write_cursor = 0;
        if self.write_flush_pending {
            ready!(Pin::new(&mut self.transport).poll_flush(cx))?;
            self.write_flush_pending = false;
        }
        Poll::Ready(Ok(()))
    }

    /// Append one frame to `write_buf`. `max_len` must be an upper bound on the encoded
    /// size; `encode` writes into the provided slice and returns the actual length (panics
    /// via `expect` if the caller under-sized `max_len`).
    pub(super) fn queue_frame(
        &mut self,
        max_len: usize,
        encode: impl FnOnce(&mut [u8]) -> Option<usize>,
    ) {
        let start = self.write_buf.len();
        self.write_buf.resize(start + max_len, 0);
        let n = encode(&mut self.write_buf[start..]).expect("buffer sized from max_len");
        self.write_buf.truncate(start + n);
        self.write_flush_pending = true;
    }

    pub(super) fn queue_settings(&mut self) {
        let settings = H2Settings::from_config(self.connection.context().config());
        self.queue_frame(frame::settings::encoded_len(&settings), |buf| {
            frame::settings::encode(&settings, buf)
        });
    }

    /// Append the 24-byte client connection preface to `write_buf`. The next outbound
    /// drain flushes it, and the `NeedsServerSettings` state follows up with our initial
    /// SETTINGS frame. Client role only.
    pub(super) fn queue_client_preface(&mut self) {
        self.write_buf
            .extend_from_slice(super::recv::CLIENT_PREFACE);
        self.write_flush_pending = true;
    }

    pub(super) fn queue_settings_ack(&mut self) {
        self.queue_frame(
            frame::settings::ACK_ENCODED_LEN,
            frame::settings::encode_ack,
        );
    }

    pub(super) fn queue_ping_ack(&mut self, opaque_data: [u8; 8]) {
        self.queue_frame(frame::ping::ENCODED_LEN, |buf| {
            frame::ping::encode(opaque_data, true, buf)
        });
    }

    pub(super) fn queue_active_ping(&mut self, opaque_data: [u8; 8]) {
        self.queue_frame(frame::ping::ENCODED_LEN, |buf| {
            frame::ping::encode(opaque_data, false, buf)
        });
    }

    pub(super) fn queue_window_update(&mut self, stream_id: u32, increment: u32) {
        self.queue_frame(frame::window_update::ENCODED_LEN, |buf| {
            frame::window_update::encode(stream_id, increment, buf)
        });
    }

    pub(super) fn queue_goaway(&mut self, last_stream_id: u32, code: H2ErrorCode) {
        self.queue_frame(frame::goaway::encoded_len(0), |buf| {
            frame::goaway::encode(last_stream_id, code, &[], buf)
        });
    }

    pub(super) fn queue_rst_stream(&mut self, stream_id: u32, code: H2ErrorCode) {
        self.queue_frame(frame::rst_stream::ENCODED_LEN, |buf| {
            frame::rst_stream::encode(stream_id, code, buf)
        });
        // Record in the ledger so subsequent frames the peer sends on this stream get
        // stream-level `STREAM_CLOSED` rather than connection-level `PROTOCOL_ERROR`.
        self.closed_streams.record(stream_id, ClosedReason::Reset);
    }
}
