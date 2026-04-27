//! Bounded ledger of recently-closed streams and how they closed.
//!
//! Consulted when a peer frame arrives on a stream id that's no longer in the active map to
//! pick the right error category per RFC 9113 §5.1. See [`ClosedReason`].

use std::collections::{HashMap, VecDeque};

/// Why a stream transitioned to the closed state — dictates the error category for any
/// subsequent frame the peer sends on that stream id (RFC 9113 §5.1):
/// - `Reset`: closed via `RST_STREAM` (either direction). Subsequent frames → stream-level
///   `STREAM_CLOSED`.
/// - `EndStream`: closed via `END_STREAM` on both sides. Subsequent frames (other than
///   `WINDOW_UPDATE` / `PRIORITY` / `RST_STREAM`) → connection-level `STREAM_CLOSED`.
///
/// Streams that were never opened and are merely implicitly closed by a higher-id
/// HEADERS (§5.1.1) don't appear in the ledger; the fall-through case there is
/// connection-level `PROTOCOL_ERROR` per §5.1.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::h2) enum ClosedReason {
    Reset,
    EndStream,
}

/// Bounded FIFO of recently-closed streams and how they closed. Consulted when a peer
/// frame arrives on a stream id that's no longer in the active map to pick the right error
/// category per RFC 9113 §5.1.
///
/// Fixed cap — this is a correctness mechanism, not a concurrency-scaled structure. A
/// well-behaved peer never sends frames on a stream it knows is closed; the ledger only
/// needs to span a handful of RTTs between our close and a misbehaving peer's stale
/// frame. Oldest entries evict on overflow; evicted lookups fall through to the §5.1.1
/// connection-level `PROTOCOL_ERROR` default.
#[derive(Debug, Default)]
pub(super) struct ClosedStreams {
    map: HashMap<u32, ClosedReason>,
    fifo: VecDeque<u32>,
}

impl ClosedStreams {
    const CAP: usize = 128;

    /// Record (or update) the close reason for `stream_id`. Idempotent on repeated calls
    /// for the same id; the most recent reason wins (a stream can be recorded as
    /// `EndStream` by `complete_and_remove_stream(Ok)` after already being recorded as
    /// `Reset` by `queue_rst_stream`, which is benign — the Reset recording is authoritative
    /// in that path because it happens first and the Ok path doesn't fire when the error
    /// path did).
    pub(super) fn record(&mut self, stream_id: u32, reason: ClosedReason) {
        if self.map.insert(stream_id, reason).is_none() {
            self.fifo.push_back(stream_id);
            while self.fifo.len() > Self::CAP {
                if let Some(old) = self.fifo.pop_front() {
                    self.map.remove(&old);
                }
            }
        }
    }

    pub(super) fn reason(&self, stream_id: u32) -> Option<ClosedReason> {
        self.map.get(&stream_id).copied()
    }
}
