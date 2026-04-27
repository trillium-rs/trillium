//! PING / PING-ACK round-trip tracking.
//!
//! [`H2Connection::send_ping`][super::H2Connection::send_ping] returns a [`SendPing`] future
//! that resolves with the round-trip time once the peer's `PING ACK` arrives. The driver-side
//! hooks ([`drain_pending_ping_outbound`][super::H2Connection::drain_pending_ping_outbound],
//! [`complete_pending_ping`][super::H2Connection::complete_pending_ping],
//! [`fail_pending_pings`][super::H2Connection::fail_pending_pings]) are the in-driver-task
//! counterparts: queue-drain, ack-arrival, connection-close.

use super::H2Connection;
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll, Waker},
    time::{Duration, Instant},
};

/// Tracks a single outstanding active PING's lifecycle.
#[derive(Debug)]
pub(crate) struct PendingPing {
    pub(crate) sent_at: Instant,
    pub(crate) waker: Option<Waker>,
    pub(crate) completed: Option<io::Result<Duration>>,
}

/// Future returned by [`H2Connection::send_ping`].
///
/// Resolves to the round-trip time once the peer's PING ACK arrives, or to an `io::Error`
/// if the connection closes first. Dropping the future before completion removes the
/// pending entry so the [`H2Connection`]'s map doesn't accumulate stale state.
#[must_use = "futures do nothing unless awaited"]
#[derive(Debug)]
pub struct SendPing<'a> {
    pub(super) connection: &'a H2Connection,
    pub(super) opaque: [u8; 8],
    /// `true` while this future still owns an entry in `pending_pings` that `Drop` must
    /// remove. Set to `false` once registration fails (duplicate opaque) or `poll` returns
    /// `Ready` with the entry removed.
    pub(super) needs_cleanup: bool,
}

impl Future for SendPing<'_> {
    type Output = io::Result<Duration>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if !this.needs_cleanup {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "PING with this opaque payload is already in flight",
            )));
        }
        let mut pending = this
            .connection
            .pending_pings
            .lock()
            .expect("pending_pings mutex poisoned");
        let entry = pending
            .get_mut(&this.opaque)
            .expect("pending_pings entry removed while SendPing future still pending");
        if let Some(result) = entry.completed.take() {
            pending.remove(&this.opaque);
            this.needs_cleanup = false;
            return Poll::Ready(result);
        }
        entry.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for SendPing<'_> {
    fn drop(&mut self) {
        if self.needs_cleanup
            && let Ok(mut pending) = self.connection.pending_pings.lock()
        {
            pending.remove(&self.opaque);
        }
    }
}

impl H2Connection {
    /// Send a `PING` frame to the peer and resolve when its `PING ACK` arrives, returning
    /// the round-trip time.
    ///
    /// `opaque` is the 8-byte payload echoed back by the peer (RFC 9113 §6.7). Caller picks
    /// the value — typically a counter or a random nonce. A `PING` whose opaque payload is
    /// already in flight on this connection resolves to `io::ErrorKind::AlreadyExists`.
    ///
    /// No internal timeout. Wrap the returned future with the runtime's
    /// `race_with_timeout` (or equivalent) to bound the wait.
    ///
    /// # Cancel safety
    ///
    /// Dropping the returned future before completion removes the pending entry from this
    /// connection's tracking map. The PING frame may still go out (or already have gone
    /// out) and the peer's ACK is silently dropped. Re-using the same `opaque` after drop
    /// is safe.
    ///
    /// # Panics
    ///
    /// Panics if any of the per-connection mutexes is poisoned (a previous thread panicked
    /// while holding the lock) — same posture as the rest of the h2 driver's mutex usage.
    pub fn send_ping(&self, opaque: [u8; 8]) -> SendPing<'_> {
        let mut pending = self
            .pending_pings
            .lock()
            .expect("pending_pings mutex poisoned");
        if pending.contains_key(&opaque) {
            return SendPing {
                connection: self,
                opaque,
                needs_cleanup: false,
            };
        }
        pending.insert(
            opaque,
            PendingPing {
                sent_at: Instant::now(),
                waker: None,
                completed: None,
            },
        );
        drop(pending);
        self.pending_ping_outbound
            .lock()
            .expect("pending_ping_outbound mutex poisoned")
            .push_back(opaque);
        self.outbound_waker.wake();
        SendPing {
            connection: self,
            opaque,
            needs_cleanup: true,
        }
    }

    /// Driver-side: drain the queue of outbound active PING opaque payloads. Called from
    /// the driver's `service_handler_signals` tick.
    pub(in crate::h2) fn drain_pending_ping_outbound(&self) -> Vec<[u8; 8]> {
        let mut queue = self
            .pending_ping_outbound
            .lock()
            .expect("pending_ping_outbound mutex poisoned");
        queue.drain(..).collect()
    }

    /// Driver-side: a `PING ACK` for the given opaque payload arrived. Marks the pending
    /// entry complete with the elapsed RTT and wakes its waker, if any. A no-op if the
    /// payload doesn't match an outstanding PING (unsolicited ACK).
    pub(in crate::h2) fn complete_pending_ping(&self, opaque: [u8; 8]) {
        let mut pending = self
            .pending_pings
            .lock()
            .expect("pending_pings mutex poisoned");
        if let Some(entry) = pending.get_mut(&opaque) {
            let elapsed = entry.sent_at.elapsed();
            entry.completed = Some(Ok(elapsed));
            if let Some(waker) = entry.waker.take() {
                waker.wake();
            }
        }
    }

    /// Driver-side: connection is closing. Complete every outstanding PING with the given
    /// error so awaiting `send_ping` futures don't block forever.
    pub(in crate::h2) fn fail_pending_pings(&self, error_kind: io::ErrorKind, message: &'static str) {
        let mut pending = self
            .pending_pings
            .lock()
            .expect("pending_pings mutex poisoned");
        for entry in pending.values_mut() {
            if entry.completed.is_none() {
                entry.completed = Some(Err(io::Error::new(error_kind, message)));
                if let Some(waker) = entry.waker.take() {
                    waker.wake();
                }
            }
        }
    }
}
