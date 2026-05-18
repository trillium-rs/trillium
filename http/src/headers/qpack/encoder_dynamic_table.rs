//! Outbound QPACK dynamic table (RFC 9204).
//!
//! Mirror of [`super::DecoderDynamicTable`] for the *encoder* side of a connection.
//! Mutations are enqueued as already-encoded encoder-stream instructions and drained by
//! [`EncoderDynamicTable::run_writer`]. Acknowledgements read from the peer's decoder
//! stream by [`EncoderDynamicTable::run_reader`] advance the Known Received Count and
//! release pinned references.
//!
//! ## Module layout
//!
//! - [`state`] — `TableState`, `insert`, eviction and reverse-index helpers. Single insert entry
//!   point; smart wire-format selection lives here, not in callers.
//! - [`encode`](self::encode) — field-section planner and emit phase. Plans entries and drives
//!   `TableState::insert` directly under the state lock; there is no public per-insert entry point
//!   on [`EncoderDynamicTable`].
//! - [`reader`](self::reader), [`writer`](self::writer) — encoder/decoder stream tasks.

use crate::{
    HttpContext,
    h3::{H3Error, H3ErrorCode, H3Settings},
    headers::{
        header_observer::HeaderCompression,
        qpack::{FieldLineValue, HeaderObserver},
        recent_pairs::RecentPairs,
    },
};
use event_listener::{Event, EventListener};
use state::TableState;
use std::sync::{Arc, Mutex};

mod connection_metrics;
mod encode;
mod reader;
mod state;
#[cfg(test)]
mod tests;
mod writer;

use connection_metrics::ConnectionMetrics;
pub(in crate::headers) use state::SectionRefs;

/// The encoder-side QPACK dynamic table for a single HTTP/3 connection.
///
/// See the module-level documentation for the overall design. Uses interior mutability so
/// it can be shared by `&self` across request-stream tasks and the encoder/decoder stream
/// task handles.
#[derive(Debug)]
pub struct EncoderDynamicTable {
    state: Mutex<TableState>,
    /// Cross-connection header-frequency observer for priming + per-section observation
    /// feedback. Shared across connections on a given listener.
    observer: Arc<HeaderObserver>,
    /// Our local upper bound on dynamic-table capacity, captured from
    /// [`HttpConfig::dynamic_table_capacity`] at construction. The negotiated capacity
    /// is `min(our_max_capacity, peer_qpack_max_table_capacity)`; consumed once by
    /// [`initialize_from_peer_settings`](Self::initialize_from_peer_settings).
    ///
    /// [`HttpConfig::dynamic_table_capacity`]: crate::HttpConfig::dynamic_table_capacity
    our_max_capacity: usize,
    /// Notified on: new op enqueued, peer ack received, failure. The encoder stream writer
    /// task awaits this to wake and drain `pending_ops`.
    event: Event,
    /// Per-connection QPACK metrics for observer/priming evaluation. Research-mode
    /// instrumentation; emitted on drop via `log::info!` at the
    /// `trillium_http::qpack_metrics` target. See
    /// [`connection_metrics`](self::connection_metrics) for the rationale.
    metrics: ConnectionMetrics,
}

impl Default for EncoderDynamicTable {
    /// Construct an empty encoder dynamic table with a fresh, empty observer and
    /// default config.
    fn default() -> Self {
        Self::new(&HttpContext::default())
    }
}

impl EncoderDynamicTable {
    /// Construct an empty encoder dynamic table for a connection running under
    /// `context`. Captures the listener's shared observer and our local max capacity.
    ///
    /// `max_capacity` and the working `capacity` both start at 0; call
    /// [`initialize_from_peer_settings`](Self::initialize_from_peer_settings) once the
    /// peer's `SETTINGS_QPACK_MAX_TABLE_CAPACITY` is known before any inserts.
    pub(crate) fn new(context: &HttpContext) -> Self {
        let observer = context.observer.clone();
        let recent_pairs_size = context.config.recent_pairs_size;
        log::trace!(
            target: "qpack_metrics",
            "new EncoderDynamicTable: observer ptr={:p} our_max_capacity={} recent_pairs_size={}",
            Arc::as_ptr(&observer),
            context.config.dynamic_table_capacity,
            recent_pairs_size,
        );
        Self {
            state: Mutex::new(TableState::new(RecentPairs::with_size(recent_pairs_size))),
            observer,
            our_max_capacity: context.config.dynamic_table_capacity,
            event: Event::new(),
            metrics: ConnectionMetrics::default(),
        }
    }
}

impl EncoderDynamicTable {
    /// Initialize the table from peer settings. Sets `max_capacity` (and the working
    /// `capacity`) to `min(our_max_capacity, peer_qpack_max_table_capacity)`, records
    /// `max_blocked_streams` from the peer's settings, and, if the chosen capacity is
    /// non-zero, enqueues a Set Dynamic Table Capacity instruction.
    ///
    /// Must be called exactly once, immediately after the peer's `SETTINGS` frame is parsed
    /// on the control stream.
    pub(crate) fn initialize_from_peer_settings(&self, peer_settings: H3Settings) {
        let peer_max_capacity =
            usize::try_from(peer_settings.qpack_max_table_capacity().unwrap_or(0))
                .unwrap_or(usize::MAX);
        let chosen = self.our_max_capacity.min(peer_max_capacity);
        let max_blocked_streams =
            usize::try_from(peer_settings.qpack_blocked_streams().unwrap_or(0))
                .unwrap_or(usize::MAX);

        let prime_entries = if chosen > 0 {
            let cap = u32::try_from(chosen).unwrap_or(u32::MAX);
            self.observer.prime(cap, HeaderCompression::Qpack)
        } else {
            Vec::new()
        };

        log::info!(
            target: "qpack_metrics",
            "initialize_from_peer_settings: peer_max_capacity={peer_max_capacity} \
             our_max_capacity={our} chosen={chosen} max_blocked_streams={max_blocked_streams} \
             prime_entries={}",
            prime_entries.len(),
            our = self.our_max_capacity,
        );

        let mut state = self.state.lock().unwrap();
        debug_assert_eq!(
            state.max_capacity, 0,
            "initialize_from_peer_settings called twice"
        );
        state.max_capacity = chosen;
        state.max_blocked_streams = max_blocked_streams;
        if chosen > 0 {
            state
                .set_capacity(chosen)
                .expect("set_capacity within max_capacity at init");
            for candidate in prime_entries {
                // Clone for the metrics record before consuming the pair in `insert`. The
                // observer returned `'static` clones so this is cheap. A `None` value is
                // a name-only candidate — primed as a `(name, "")` dynamic-table entry.
                let name_for_metrics = candidate.name.clone();
                let value_for_insert = candidate
                    .value
                    .clone()
                    .unwrap_or(FieldLineValue::Static(b""));
                let value_for_metrics = value_for_insert.clone();
                let kind = if candidate.value.is_some() {
                    "full-pair"
                } else {
                    "name-only"
                };
                let entry_size = candidate.name.len() + value_for_insert.as_bytes().len() + 32;
                match state.insert(candidate.name, value_for_insert, None) {
                    Ok(abs_idx) => {
                        state.primed_bytes = state.primed_bytes.saturating_add(entry_size);
                        let wire_bytes = state
                            .pending_ops
                            .back()
                            .map_or(0, |b| u64::try_from(b.len()).unwrap_or(u64::MAX));
                        log::info!(
                            target: "qpack_metrics",
                            "priming insert ({kind}): abs_idx={abs_idx} wire_bytes={wire_bytes} \
                             name={:?} value={:?}",
                            name_for_metrics,
                            String::from_utf8_lossy(value_for_metrics.as_bytes()),
                        );
                        self.metrics.record_primed_insert(
                            abs_idx,
                            name_for_metrics,
                            value_for_metrics,
                            wire_bytes,
                        );
                    }
                    Err(err) => {
                        log::debug!("qpack observer priming insert failed: {err:?}");
                        break;
                    }
                }
            }
            drop(state);
            self.event.notify(usize::MAX);
        }
    }

    /// Take all currently-queued encoder-stream instructions. Called by the writer task
    /// each time it wakes; it flushes the returned bytes and then awaits [`listen`](Self::listen).
    pub(in crate::headers) fn drain_pending_ops(&self) -> Vec<Vec<u8>> {
        self.state.lock().unwrap().pending_ops.drain(..).collect()
    }

    /// Create an [`EventListener`] that resolves on the next state change (op enqueued,
    /// peer ack received, or failure).
    pub(in crate::headers) fn listen(&self) -> EventListener {
        self.event.listen()
    }

    /// Record a Section Acknowledgement received from the peer's decoder stream. Pops the
    /// oldest outstanding section for `stream_id` and, if its `required_insert_count`
    /// exceeds the current known-received count, advances the latter. Returns an error if
    /// no section is outstanding for this stream (protocol error by the peer).
    pub(in crate::headers) fn on_section_ack(&self, stream_id: u64) -> Result<(), H3Error> {
        let mut state = self.state.lock().unwrap();
        let section = state
            .outstanding_sections
            .get_mut(&stream_id)
            .and_then(std::collections::VecDeque::pop_front)
            .ok_or(H3ErrorCode::QpackDecoderStreamError)?;
        if state
            .outstanding_sections
            .get(&stream_id)
            .is_some_and(std::collections::VecDeque::is_empty)
        {
            state.outstanding_sections.remove(&stream_id);
        }
        if section.required_insert_count > state.known_received_count {
            state.known_received_count = section.required_insert_count;
        }
        drop(state);
        self.event.notify(usize::MAX);
        Ok(())
    }

    /// Record a Stream Cancellation received from the peer's decoder stream. Drops all
    /// outstanding sections for `stream_id` without advancing known-received.
    pub(in crate::headers) fn on_stream_cancel(&self, stream_id: u64) {
        let mut state = self.state.lock().unwrap();
        state.outstanding_sections.remove(&stream_id);
        drop(state);
        self.event.notify(usize::MAX);
    }

    /// Record an Insert Count Increment received from the peer's decoder stream. Advances
    /// known-received by `increment`. Returns an error if this would exceed `insert_count`
    /// (protocol error by the peer).
    pub(in crate::headers) fn on_insert_count_increment(
        &self,
        increment: u64,
    ) -> Result<(), H3Error> {
        if increment == 0 {
            return Err(H3ErrorCode::QpackDecoderStreamError.into());
        }
        let mut state = self.state.lock().unwrap();
        let new_value = state
            .known_received_count
            .checked_add(increment)
            .ok_or(H3ErrorCode::QpackDecoderStreamError)?;
        if new_value > state.insert_count {
            return Err(H3ErrorCode::QpackDecoderStreamError.into());
        }
        state.known_received_count = new_value;
        drop(state);
        self.event.notify(usize::MAX);
        Ok(())
    }

    /// Signal that the encoder or decoder stream has failed. Wakes the writer so it can
    /// observe `failed()` and exit.
    pub(crate) fn fail(&self, code: H3ErrorCode) {
        self.state.lock().unwrap().failed = Some(code);
        self.event.notify(usize::MAX);
    }

    /// Returns `Some(code)` if the table has been marked failed.
    pub(in crate::headers) fn failed(&self) -> Option<H3ErrorCode> {
        self.state.lock().unwrap().failed
    }
}

impl Drop for EncoderDynamicTable {
    /// Fold this connection's accumulated header observations into the shared
    /// cross-connection observer. The only place we touch the shared observer's
    /// counters from the encode path; runs once per connection, no contention with
    /// the encode hot path. A poisoned `state` mutex is silently skipped — the lost
    /// contribution is one connection's worth of counts.
    fn drop(&mut self) {
        if let Ok(state) = self.state.lock() {
            self.observer.fold_connection(&state.accum);
        }
    }
}
