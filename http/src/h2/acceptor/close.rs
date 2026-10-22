use super::{
    H2Driver,
    constants::READ_WINDOW,
    types::{CloseOutcome, DriverState},
};
use crate::{
    Conn,
    h2::{H2Error, H2ErrorCode, stream_state::StreamEvent, transport::H2Transport},
};
use futures_lite::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

impl<T> H2Driver<T>
where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    /// Convert the current `close_outcome` into the terminal return of [`Self::drive`]. Must
    /// only be called after outbound bytes have been flushed. Graceful closes return `None`;
    /// errors surface as a final `Some(Err(...))` before subsequent polls return `None`.
    pub(super) fn finish_with_current_outcome(
        &mut self,
    ) -> Option<Result<Conn<H2Transport>, H2Error>> {
        self.finished = true;
        // Shut the connection swansong down on every exit path, not just peer GOAWAY:
        // it is the signal reuse decisions key on (`open_stream`, `can_open_stream`, and
        // pooling clients' classify). A driver that dies on an I/O error or peer FIN while
        // leaving the swansong running turns the connection into a landmine — a pool hands
        // it out as available, `open_stream` publishes a stream no driver will ever
        // service, and the response waiter parks forever. Must happen *before* the shared-
        // map walk below: `open_stream` checks the swansong under the streams lock, so any
        // stream that slips in concurrently either observes the shutdown and is refused,
        // or was published before we take the lock and gets reset by the walk.
        self.connection.shut_down();
        // Complete every outstanding `H2Connection::send_ping` future with an error so
        // awaiting callers don't block forever. Safe to call regardless of outcome —
        // a no-op if no pings are in flight.
        self.connection.fail_pending_pings(
            io::ErrorKind::ConnectionAborted,
            "h2 connection closed before PING ACK",
        );
        // Wake any `PeerSettings` waiters so a peer that disconnects without ever sending
        // SETTINGS doesn't strand them. Their `poll` rechecks swansong state and returns
        // Ready; the caller's follow-up operation surfaces the connection-closed error.
        self.connection.wake_peer_settings_waiters();
        // Resolve every still-live stream's recv-side waiters. A connection that dies with
        // an in-flight stream (server GOAWAY + close, peer FIN, I/O error) leaves any task
        // parked on the response — `response_headers`, a body `poll_read`, an upgrade
        // `poll_write` — with no other wake source. Without this a client request hangs
        // forever on a graceful server shutdown. Mirror the per-stream RST teardown:
        // terminal `Reset` (recv reports eof → `ResponseHeaders` yields `ConnectionAborted`,
        // reads return EOF, writes `BrokenPipe`) + the same waker fan-out.
        let reset_code = match &self.close_outcome {
            Some(CloseOutcome::Protocol(code)) => *code,
            _ => H2ErrorCode::NoError,
        };
        // Walk the *shared* map, not the driver-private mirror: a client stream published
        // between the driver's last pickup pass and this teardown exists only in the shared
        // map, and every conn-task waiter (`response_headers`, body reads, `SubmitSend`)
        // reaches its stream through the shared map — so this walk covers all of them.
        for state in self.connection.streams_lock().values() {
            // Move each still-live stream to `Closed{Reset}` (a no-op on streams already closed, so
            // an existing reason isn't clobbered), then fan out every recv/send waker so parked
            // tasks observe the close instead of hanging.
            let _ = state.apply_event(StreamEvent::RecvReset(reset_code));
            state.recv.waker.wake();
            state.recv.response_headers_waker.wake();
            state.send.outbound_write_waker.wake();
            // A handler already parked in `SubmitSend` (response staged, awaiting the driver to
            // frame it) needs this wake to re-poll and observe the now-reset stream — the recv
            // fan-out above doesn't reach the send-completion waiter.
            state.send.completion_waker.wake();
        }
        match self.close_outcome.take() {
            None | Some(CloseOutcome::Graceful) => None,
            Some(CloseOutcome::Protocol(code)) => Some(Err(H2Error::Protocol(code))),
            Some(CloseOutcome::Io(e)) => Some(Err(H2Error::Io(e))),
        }
    }

    /// Enter the closing state: record the outcome and queue a GOAWAY (only for outcomes
    /// that warrant one). The main loop will drain `write_buf` and then finish.
    pub(super) fn begin_close(&mut self, outcome: CloseOutcome) {
        // Idempotent: with the recv pump now running in Closing (so we keep
        // decoding inbound frames for in-flight streams across GOAWAY), a peer
        // GOAWAY arriving after we've already begun closing would otherwise
        // re-queue our own GOAWAY and re-enter Closing, ping-ponging forever
        // with a peer that mirrors the behavior.
        if self.state == DriverState::Closing || self.state == DriverState::Drained {
            log::trace!(
                "h2 driver: begin_close({outcome:?}) — already in {:?}, ignoring",
                self.state,
            );
            return;
        }
        // Don't overwrite a prior outcome (e.g. if an error fires in the middle of a
        // graceful shutdown, keep the error).
        let code = match &outcome {
            CloseOutcome::Graceful => Some(H2ErrorCode::NoError),
            CloseOutcome::Protocol(code) => Some(*code),
            CloseOutcome::Io(_) => None,
        };
        let reason = match &outcome {
            CloseOutcome::Graceful => "graceful close",
            CloseOutcome::Protocol(_) => "protocol error",
            CloseOutcome::Io(_) => "i/o error",
        };
        if self.close_outcome.is_none() {
            self.close_outcome = Some(outcome);
        }
        if let Some(code) = code {
            self.queue_goaway(self.last_peer_stream_id, code);
        }
        self.set_state(DriverState::Closing, reason);
    }

    /// Log which in-flight streams are blocking the `Closing → Drained` transition.
    /// Called from the closing-state check when at least one predicate (`has_active_send_cursors`
    /// or `has_pending_recv`) is still true, so a trace log shows exactly which streams the
    /// driver is waiting on.
    pub(super) fn log_closing_blockers(&self) {
        if !log::log_enabled!(log::Level::Trace) {
            return;
        }
        for (id, entry) in &self.streams {
            let lifecycle = *entry.shared.lifecycle_lock();
            let queued = !entry
                .shared
                .send
                .queue
                .lock()
                .expect("send queue mutex poisoned")
                .is_empty();
            if entry.send.is_some() || queued || !lifecycle.recv_closed() {
                log::trace!(
                    "h2 driver: Closing — stream {id} blocking drain (lifecycle={lifecycle:?}, \
                     cursor_present={}, queued={queued})",
                    entry.send.is_some(),
                );
            }
        }
    }

    /// Post-GOAWAY, drain whatever inbound bytes are *immediately* available from the
    /// peer so our Drop sends a clean FIN (no unread data → no TCP RST) while the peer
    /// sees the GOAWAY we just emitted. Read loops internally: consume each Ready chunk,
    /// discard it, ask for more. Exits as soon as the transport returns `Pending` (no
    /// bytes available right now) OR `Ready(0)` (peer FIN already arrived) OR any error.
    ///
    /// Does **not** register the waker on `Pending` — we're actively closing, not
    /// observing the peer. Bytes the peer sends after this returns are [`Self::close`]'s to
    /// wait out; dropping the transport instead answers them with a TCP reset.
    ///
    /// Returning `Ready(())` unconditionally (no `Pending` case) lets the caller finalize
    /// immediately. The `Poll` wrapper is kept for symmetry with the rest of the driver's
    /// poll-style methods.
    pub(super) fn poll_drain_peer(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        // A peer flooding us with bytes could keep this loop going a long time. Cap it
        // so a pathological client can't pin our close-out forever.
        const MAX_DISCARD_ITERATIONS: usize = 256;
        // Lightweight scratch — we're throwing it away. 512 balances "drain in few
        // iterations" against "don't hold a large buffer for a rare path."
        let mut scratch = [0u8; 512];
        for _ in 0..MAX_DISCARD_ITERATIONS {
            // We pass `cx` through for the benefit of the transport's `poll_read` contract,
            // but we *interpret* `Pending` as "done draining" rather than parking on it —
            // we're actively closing, not observing.
            match Pin::new(&mut self.transport).poll_read(cx, &mut scratch) {
                Poll::Ready(Ok(0) | Err(_)) | Poll::Pending => {
                    return Poll::Ready(());
                }
                Poll::Ready(Ok(_)) => {}
            }
        }
        Poll::Ready(())
    }

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
