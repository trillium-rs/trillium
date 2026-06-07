// Adapted from golang.org/x/net/http2/flow.go.
//
// Copyright 2014 The Go Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the licences/go file.

//! Inbound (receive-side) flow-control accounting for a single scope — one stream, or the whole
//! connection.
//!
//! Models the receive window the way RFC 9113 §6.9 flow control actually works from a receiver's
//! point of view: we grant the peer a window, the peer spends it by sending DATA, and we top it
//! back up as the handler drains buffered bytes. Two accumulators make that exact:
//!
//! - `avail` — what the peer may still send *right now* (everything we've granted on the wire,
//!   minus the DATA we've seen). This is the value we enforce against: a peer that sends past it
//!   has overrun the window.
//! - `unsent` — credit the handler has earned (by consuming buffered bytes) that we haven't yet put
//!   on the wire as a `WINDOW_UPDATE`. Batching small updates here avoids a `WINDOW_UPDATE` storm
//!   on every read.
//!
//! The window never exceeds `target`, and the key invariant `avail + buffered ≤ target` holds at
//! all times — so `target` is simultaneously the flow-control window *and* the per-scope buffer
//! bound. There is no separate cap to reconcile.

/// Minimum accumulated credit before we emit a `WINDOW_UPDATE`, unless the pending credit would
/// at least double the peer's current window. Batches sub-threshold updates so a handler reading a
/// body in small chunks doesn't trigger a `WINDOW_UPDATE` per read.
pub(super) const INFLOW_MIN_REFRESH: i64 = 4 << 10;

/// Receive-side flow-control window for one scope. See the [module docs][self].
#[derive(Clone, Copy, Debug)]
pub(super) struct Inflow {
    /// What the peer may still send: granted-and-flushed window minus DATA observed.
    avail: i64,
    /// Earned-but-unflushed credit, accumulated until the hysteresis threshold in [`Inflow::add`].
    unsent: i64,
    /// The window size we top back up to; also the worst-case buffer bound for this scope.
    target: i64,
}

impl Inflow {
    /// Open a scope with the peer immediately able to send `target` bytes (the advertised initial
    /// window for a stream, or the post-raise connection window).
    pub(super) const fn new(target: i64) -> Self {
        Self {
            avail: target,
            unsent: 0,
            target,
        }
    }

    /// Account for `n` inbound DATA octets (the flow-controlled length: payload + pad-length byte +
    /// padding). Returns `false` if `n` exceeds the window we granted — a flow-control violation
    /// the caller escalates to a `FLOW_CONTROL_ERROR`. A compliant peer never trips this, since
    /// we only ever grant window via `WINDOW_UPDATE`s we've actually sent.
    pub(super) fn take(&mut self, n: i64) -> bool {
        if n > self.avail {
            return false;
        }
        self.avail -= n;
        true
    }

    /// Record `n` octets drained by the handler and return the `WINDOW_UPDATE` increment to emit
    /// (`0` to buffer it for later). Credit accumulates in `unsent` and is flushed — re-granted to
    /// the peer — once it reaches [`INFLOW_MIN_REFRESH`] or would at least double the peer's
    /// current window (the `unsent ≥ avail` arm, which also forces an immediate flush when the
    /// peer is fully blocked at `avail == 0`).
    pub(super) fn add(&mut self, n: i64) -> i64 {
        debug_assert!(n >= 0, "flow-control credit must be non-negative");
        self.unsent += n;
        if self.unsent < INFLOW_MIN_REFRESH && self.unsent < self.avail {
            return 0;
        }
        let increment = self.unsent;
        self.avail += increment;
        self.unsent = 0;
        increment
    }

    /// Promote the scope to a larger `new_target`, returning the `WINDOW_UPDATE` increment that
    /// grows the peer's window to it immediately (bypassing [`add`][Inflow::add]'s hysteresis — a
    /// promotion is a deliberate one-time grant we want on the wire now). Returns `0` if
    /// `new_target` is not larger than the current target; the window is never lowered.
    ///
    /// Adding the target *delta* to `avail` preserves `avail + buffered ≤ target` against the new
    /// target without needing to know `buffered`: the old sum was within the old target, and both
    /// sides grow by the same delta.
    pub(super) fn raise_target(&mut self, new_target: i64) -> i64 {
        if new_target <= self.target {
            return 0;
        }
        let delta = new_target - self.target;
        self.target = new_target;
        self.avail += delta;
        delta
    }
}

#[cfg(test)]
mod tests {
    use super::{INFLOW_MIN_REFRESH, Inflow};

    #[test]
    fn take_spends_window_and_reports_overrun() {
        let mut f = Inflow::new(100);
        assert!(f.take(60));
        assert_eq!(f.avail, 40);
        // Exactly draining the window is fine.
        assert!(f.take(40));
        assert_eq!(f.avail, 0);
        // One byte past it is an overrun.
        assert!(!f.take(1));
        assert_eq!(f.avail, 0, "a rejected take leaves the window unchanged");
    }

    #[test]
    fn add_buffers_below_threshold() {
        let mut f = Inflow::new(1 << 20);
        f.take(1 << 20); // avail now huge..0? no: target 1MiB, take all -> avail 0
        // Refill avail so the doubling arm doesn't force a flush; check pure threshold batching.
        f.raise_target(2 << 20); // avail back to ~1MiB
        let small = INFLOW_MIN_REFRESH - 1;
        assert_eq!(f.add(small), 0, "sub-threshold credit is buffered");
        assert_eq!(
            f.add(1),
            INFLOW_MIN_REFRESH,
            "crossing the threshold flushes all buffered credit"
        );
    }

    #[test]
    fn add_flushes_when_it_would_double_the_window() {
        let mut f = Inflow::new(100);
        f.take(90); // avail = 10, well below INFLOW_MIN_REFRESH
        // 10 bytes of credit is < MIN_REFRESH but == avail, so it at least doubles -> flush now.
        assert_eq!(f.add(10), 10);
        assert_eq!(f.avail, 20);
    }

    #[test]
    fn add_flushes_immediately_when_peer_is_blocked() {
        let mut f = Inflow::new(100);
        f.take(100); // avail = 0, peer fully blocked
        // Any credit at all must go out now — `unsent < avail` is `1 < 0` = false.
        assert_eq!(f.add(1), 1);
        assert_eq!(f.avail, 1);
    }

    #[test]
    fn raise_target_grows_window_by_the_delta() {
        let mut f = Inflow::new(256);
        f.take(200); // avail = 56
        let inc = f.raise_target(1024);
        assert_eq!(inc, 768, "increment is the target delta");
        assert_eq!(f.avail, 56 + 768);
        assert_eq!(f.target, 1024);
    }

    #[test]
    fn raise_target_never_lowers() {
        let mut f = Inflow::new(1024);
        assert_eq!(f.raise_target(512), 0);
        assert_eq!(f.raise_target(1024), 0);
        assert_eq!(f.target, 1024);
        assert_eq!(f.avail, 1024);
    }

    #[test]
    fn invariant_avail_plus_buffered_stays_within_target() {
        // Simulate a stream: peer sends, handler drains, we promote, repeat. Track buffered
        // explicitly and assert the invariant holds throughout.
        let mut f = Inflow::new(256);
        let mut buffered: i64 = 0;

        // Peer fills the initial window.
        assert!(f.take(256));
        buffered += 256;
        assert!(f.avail + buffered <= f.target);

        // Handler drains 100; credit flushes (100 < MIN_REFRESH, but avail==0 forces a flush).
        f.add(100);
        buffered -= 100;
        // The flushed credit was granted back to the peer; it may now send that much more, all of
        // which could land in the buffer — the invariant must still hold.
        assert!(f.avail + buffered <= f.target, "after refill");

        // Promote to the read-target and let the peer fill it.
        f.raise_target(1024);
        let room = f.avail;
        assert!(f.take(room));
        buffered += room;
        assert!(f.avail + buffered <= f.target, "after promotion + fill");
        assert_eq!(f.avail, 0);
    }
}
