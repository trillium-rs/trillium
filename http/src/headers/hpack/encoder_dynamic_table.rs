//! Dynamic-table-aware HPACK encoder (RFC 7541).
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

use crate::headers::header_observer::HeaderObserver;
use state::TableState;
use std::sync::Arc;

mod encode;
mod state;
#[cfg(test)]
mod tests;

/// Per-connection HPACK encoder.
///
/// Construct one per HTTP/2 connection (server or client). The operational table size
/// starts at 0 (encoder reduces to static-or-literal) until the peer's
/// `SETTINGS_HEADER_TABLE_SIZE` arrives via [`Self::set_protocol_max_size`]. At that
/// point, the operational size is raised to `min(local_preferred_size, peer_advertised)`
/// and a Dynamic Table Size Update is queued for the next encode call. Subsequent peer
/// SETTINGS changes flow through the same path.
///
/// This "wait for peer" posture differs from the spec's stated default of 4096 (we
/// *could* use the dynamic table from frame zero) but mirrors QPACK's
/// peer-advertised-capacity model, removes a client-side race where pre-SETTINGS HEADERS
/// would be emitted assuming 4096 against a peer that intends to advertise less, and
/// unifies the mental model across HPACK and QPACK.
///
/// The cross-connection header observer is shared via `Arc` across all connections on a
/// listener; this encoder folds its per-connection observation accumulator into the
/// shared observer in [`Drop`].
#[derive(Debug)]
pub(crate) struct HpackEncoder {
    state: TableState,
    /// Cross-connection header observer. Shared across connections on a listener;
    /// consulted on each line for the `should_index` first-sight promotion gate.
    /// Has its own internal locking; independent of this encoder's state.
    observer: Arc<HeaderObserver>,
}

impl HpackEncoder {
    /// Construct a new HPACK encoder with the given local preferred dynamic-table capacity
    /// (bytes) and recent-pairs ring size. When `recent_pairs_auto` is set, the ring size
    /// and insert threshold are re-derived from the operational table size on each
    /// [`Self::set_protocol_max_size`], and `recent_pairs_size` only sizes the
    /// pre-SETTINGS ring (inert — the operational size is 0, so nothing inserts).
    ///
    /// The encoder's operational size starts at 0 and is raised on the first call to
    /// [`Self::set_protocol_max_size`] (typically driven by the peer's
    /// `SETTINGS_HEADER_TABLE_SIZE`). `local_preferred_size = 0` is a valid input: the
    /// encoder will never insert regardless of what the peer advertises (every candidate
    /// fails the "entry alone exceeds `max_size`" check).
    pub(crate) fn new(
        observer: Arc<HeaderObserver>,
        local_preferred_size: usize,
        recent_pairs_size: usize,
        recent_pairs_auto: bool,
    ) -> Self {
        Self {
            state: TableState::new(local_preferred_size, recent_pairs_size, recent_pairs_auto),
            observer,
        }
    }

    /// Apply the peer's advertised `SETTINGS_HEADER_TABLE_SIZE`. The encoder's operational
    /// size becomes `min(local_preferred_size, peer_advertised)`; if that changed, the
    /// table is shrunk if needed and a Dynamic Table Size Update is queued for emission at
    /// the start of the next [`Self::encode`] call. Idempotent: a no-op when the
    /// operational size is unchanged.
    pub(crate) fn set_protocol_max_size(&mut self, peer_advertised: usize) {
        self.state.set_protocol_max_size(peer_advertised);
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
