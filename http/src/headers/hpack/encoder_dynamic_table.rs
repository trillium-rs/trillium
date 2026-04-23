//! Dynamic-table-aware HPACK encoder (RFC 7541 §6).
//!
//! Per-connection encoder owned by the H2 driver task. Each HEADERS block is encoded via
//! [`HpackEncoder::encode`] in a single synchronous pass at submission-pickup time. The
//! encoder is held by `&mut` on the driver — nothing else reaches it, so the table state
//! is just a plain field with no synchronization.
//!
//! ## Module layout
//!
//! - [`state`] — `TableState`, `insert`, FIFO eviction, reverse-index helpers.
//! - [`encode`] — the per-line decision walk and wire-format helpers.
//!
//! ## Distinct from QPACK
//!
//! HPACK has no encoder stream, no Known Received Count, no outstanding-section
//! pinning, no blocked-streams budget, and no Duplicate instruction. Inserts are
//! atomic with the field-line representation that triggers them (§6.2.1), and
//! references in a block are interpreted in order against the table state at that
//! point in the block.

use crate::headers::header_observer::HeaderObserver;
use state::TableState;
use std::sync::Arc;

mod encode;
mod state;
#[cfg(test)]
mod tests;

/// Per-connection HPACK encoder.
///
/// Construct one per HTTP/2 connection (server or client). The dynamic table is
/// initialized at the peer's advertised `SETTINGS_HEADER_TABLE_SIZE` (or our local
/// configured ceiling, whichever is smaller — the caller picks). The cross-connection
/// header observer is shared via `Arc` across all connections on a listener; this
/// encoder folds its per-connection observation accumulator into the shared observer
/// in [`Drop`].
#[derive(Debug)]
pub(crate) struct HpackEncoder {
    state: TableState,
    /// Cross-connection header observer. Shared across connections on a listener;
    /// consulted on each line for the `should_index` first-sight promotion gate.
    /// Has its own internal locking; independent of this encoder's state.
    observer: Arc<HeaderObserver>,
}

impl HpackEncoder {
    /// Construct a new HPACK encoder with the given dynamic-table capacity (in bytes,
    /// per RFC 7541 §4.1) and the given recent-pairs ring size. The observer is
    /// shared across connections on a listener.
    ///
    /// `max_table_size = 0` is a valid input: the encoder will never insert (every
    /// candidate fails §4.4's "entry alone exceeds `max_size`" check), reducing to the
    /// static-or-literal shape.
    pub(crate) fn new(
        observer: Arc<HeaderObserver>,
        max_table_size: usize,
        recent_pairs_size: usize,
    ) -> Self {
        Self {
            state: TableState::new(max_table_size, recent_pairs_size),
            observer,
        }
    }
}

impl Drop for HpackEncoder {
    /// Fold this connection's accumulated header observations into the shared
    /// cross-connection observer. The only place we touch the shared observer's
    /// counters from this encoder; runs once per connection.
    fn drop(&mut self) {
        self.observer.fold_connection(&self.state.accum);
    }
}
