//! Bounded ledger of recently-closed streams and how they closed.
//!
//! Consulted when DATA or HEADERS arrive on a stream id that's no longer in the active map, to
//! decide between ignoring the frame and which error it draws. See [`ClosedStream`]. Whether a
//! stream was ever opened comes from the driver's stream-id watermarks, not this ledger, so
//! eviction never makes a closed stream look idle.

use hashbrown::HashMap;
use std::collections::VecDeque;

/// How a recently-closed stream closed, which decides how later DATA and HEADERS on its id are
/// handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::h2) struct ClosedStream {
    /// Why it closed. A reset outranks a clean close.
    pub(in crate::h2) reason: ClosedReason,

    /// Whether our `RST_STREAM` is the latest reset on this stream. The peer may have sent frames
    /// before seeing it, so those are ignored. A later peer reset clears this: anything the peer
    /// sends after its own reset is its error, answered with one `RST_STREAM(STREAM_CLOSED)` —
    /// which sets this again.
    pub(in crate::h2) ignore_late_frames: bool,
}

/// Why a stream transitioned to the closed state. Unless [`ClosedStream::ignore_late_frames`]
/// applies:
/// - `Reset`: closed via `RST_STREAM` → later DATA or HEADERS draw stream-level `STREAM_CLOSED`.
/// - `EndStream`: closed via `END_STREAM` on both sides. DATA → stream-level `STREAM_CLOSED`;
///   HEADERS on a server → connection-level `STREAM_CLOSED`.
///
/// Streams that were never opened and are merely implicitly closed by a higher-id HEADERS
/// don't appear in the ledger; for a server, HEADERS there is connection-level `PROTOCOL_ERROR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::h2) enum ClosedReason {
    Reset,
    EndStream,
}

/// Bounded FIFO of recently-closed streams and how they closed.
///
/// Fixed cap — this is a correctness mechanism, not a concurrency-scaled structure. It only
/// needs to span a handful of RTTs between a close and the peer's stale frames; past that,
/// RFC 9113 §5.1 lets an endpoint stop ignoring frames after its reset. Oldest entries evict on
/// overflow, and evicted lookups fall through to the defaults described on [`ClosedReason`].
#[derive(Debug, Default)]
pub(super) struct ClosedStreams {
    map: HashMap<u32, ClosedStream>,
    fifo: VecDeque<u32>,
}

impl ClosedStreams {
    pub(super) const CAP: usize = 128;

    /// Record that `stream_id` closed for `reason`. Doesn't change whose reset was last.
    pub(super) fn record_close(&mut self, stream_id: u32, reason: ClosedReason) {
        self.entry(stream_id, reason);
    }

    /// Record that we sent `RST_STREAM` on `stream_id`.
    pub(super) fn record_local_reset(&mut self, stream_id: u32) {
        self.entry(stream_id, ClosedReason::Reset)
            .ignore_late_frames = true;
    }

    /// Record that the peer sent `RST_STREAM` on `stream_id`.
    pub(super) fn record_peer_reset(&mut self, stream_id: u32) {
        self.entry(stream_id, ClosedReason::Reset)
            .ignore_late_frames = false;
    }

    pub(super) fn get(&self, stream_id: u32) -> Option<ClosedStream> {
        self.map.get(&stream_id).copied()
    }

    fn entry(&mut self, stream_id: u32, reason: ClosedReason) -> &mut ClosedStream {
        if !self.map.contains_key(&stream_id) {
            self.fifo.push_back(stream_id);
            while self.fifo.len() > Self::CAP {
                if let Some(old) = self.fifo.pop_front() {
                    self.map.remove(&old);
                }
            }
        }
        let closed = self.map.entry(stream_id).or_insert(ClosedStream {
            reason,
            ignore_late_frames: false,
        });
        if reason == ClosedReason::Reset {
            closed.reason = ClosedReason::Reset;
        }
        closed
    }
}
