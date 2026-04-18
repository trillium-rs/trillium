//! Outbound QPACK dynamic table (RFC 9204 §3.2).
//!
//! Mirror of [`super::DecoderDynamicTable`] for the *encoder* side of a connection. Mutations
//! are enqueued as already-encoded encoder-stream instructions and drained by
//! [`EncoderDynamicTable::run_writer`]. Acknowledgements read from the peer's decoder stream
//! by [`EncoderDynamicTable::run_reader`] advance the Known Received Count and release
//! pinned references.
//!
//! ## Module layout
//!
//! - [`state`] — `TableState`, `insert`, eviction and reverse-index helpers. Single insert entry
//!   point; smart wire-format selection lives here, not in callers.
//! - [`encode`](self::encode) — §4.5 field-section planner and emit phase.
//! - [`reader`](self::reader), [`writer`](self::writer) — encoder/decoder stream tasks.
//!
//! Policy code drives the table through [`EncoderDynamicTable::insert`] and
//! [`EncoderDynamicTable::set_capacity`]. The choice of §3.2 wire format (duplicate /
//! literal name / static name ref / dynamic name ref) is internal to `insert` — callers
//! describe intent, not encoding.

use crate::{
    h3::{H3Error, H3ErrorCode, H3Settings},
    headers::qpack::{FieldLineValue, entry_name::QpackEntryName},
};
use event_listener::{Event, EventListener};
use state::TableState;
use std::sync::Mutex;

mod encode;
mod reader;
mod state;
#[cfg(test)]
mod tests;
mod writer;

pub(in crate::headers) use state::SectionRefs;

/// The encoder-side QPACK dynamic table for a single HTTP/3 connection.
///
/// See the module-level documentation for the overall design. Uses interior mutability so
/// it can be shared by `&self` across request-stream tasks and the encoder/decoder stream
/// task handles.
#[derive(Debug)]
pub struct EncoderDynamicTable {
    state: Mutex<TableState>,
    /// Notified on: new op enqueued, peer ack received, failure. The encoder stream writer
    /// task awaits this to wake and drain `pending_ops`.
    event: Event,
}

impl Default for EncoderDynamicTable {
    /// Construct an empty encoder dynamic table. `max_capacity` and the working `capacity`
    /// both start at 0; call [`initialize_from_peer_settings`](Self::initialize_from_peer_settings)
    /// once the peer's `SETTINGS_QPACK_MAX_TABLE_CAPACITY` is known before any inserts.
    fn default() -> Self {
        Self {
            state: Mutex::new(TableState::new()),
            event: Event::new(),
        }
    }
}

impl EncoderDynamicTable {
    /// Initialize the table from peer settings. Sets `max_capacity` (and the working
    /// `capacity`) to `min(our_max_capacity, peer_qpack_max_table_capacity)`, records
    /// `max_blocked_streams` from the peer's settings, and, if the chosen capacity is
    /// non-zero, enqueues a Set Dynamic Table Capacity instruction (RFC 9204 §3.2.1,
    /// §4.3.1).
    ///
    /// Must be called exactly once, immediately after the peer's `SETTINGS` frame is parsed
    /// on the control stream.
    pub(crate) fn initialize_from_peer_settings(
        &self,
        our_max_capacity: usize,
        peer_settings: H3Settings,
    ) {
        let peer_max_capacity =
            usize::try_from(peer_settings.qpack_max_table_capacity().unwrap_or(0))
                .unwrap_or(usize::MAX);
        let chosen = our_max_capacity.min(peer_max_capacity);
        let max_blocked_streams =
            usize::try_from(peer_settings.qpack_blocked_streams().unwrap_or(0))
                .unwrap_or(usize::MAX);
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
            drop(state);
            self.event.notify(usize::MAX);
        }
    }

    /// Insert `(name, value)` into the dynamic table.
    ///
    /// Single insertion entry point. The §3.2 wire format (Duplicate when the entry already
    /// matches a live entry / static name ref / dynamic name ref / literal name) is chosen
    /// inside [`TableState::insert`] based on current state; callers describe intent only.
    ///
    /// Returns the absolute index of the freshly-inserted entry.
    ///
    /// # Errors
    ///
    /// Returns `H3Error` if the entry doesn't fit under capacity or eviction would drop a
    /// pinned entry.
    #[allow(dead_code)] // wired into encode path in a follow-up commit
    pub(super) fn insert(
        &self,
        name: QpackEntryName<'_>,
        value: FieldLineValue<'_>,
    ) -> Result<u64, H3Error> {
        let mut state = self.state.lock().unwrap();
        let abs_idx = state.insert(name, value, None)?;
        drop(state);
        self.event.notify(usize::MAX);
        Ok(abs_idx)
    }

    /// Enqueue a Set Dynamic Table Capacity instruction (RFC 9204 §3.2.1, §4.3.1).
    ///
    /// Evicts oldest entries that no longer fit under the new capacity, respecting the
    /// outstanding-sections pin floor. Returns an error if `new_capacity > max_capacity`
    /// or if eviction would require dropping a pinned entry.
    #[allow(dead_code)] // exercised by tests; first production caller lands with policy integration
    pub(in crate::headers) fn set_capacity(&self, new_capacity: usize) -> Result<(), H3Error> {
        let mut state = self.state.lock().unwrap();
        state.set_capacity(new_capacity)?;
        drop(state);
        self.event.notify(usize::MAX);
        Ok(())
    }

    /// Look up a dynamic-table entry whose name and value both match. Returns the absolute
    /// index of the latest such entry, or `None` if no live entry has this `(name, value)`.
    ///
    /// `value` is taken as `&[u8]` so the encode path can probe the map with
    /// `FieldSection`-sourced `&str` bytes without allocating a `HeaderValue`.
    pub(in crate::headers) fn find_full_match(
        &self,
        name: &QpackEntryName,
        value: &[u8],
    ) -> Option<u64> {
        let state = self.state.lock().unwrap();
        state
            .by_name
            .get(name)
            .and_then(|index| index.by_value.get(value).copied())
    }

    /// Look up a dynamic-table entry whose name matches (value may differ). Returns the
    /// absolute index of the latest such entry, or `None` if no live entry has this name.
    pub(in crate::headers) fn find_name_match(&self, name: &QpackEntryName) -> Option<u64> {
        let state = self.state.lock().unwrap();
        state.by_name.get(name).map(|index| index.latest_any)
    }

    /// Number of distinct streams with at least one outstanding section whose Required
    /// Insert Count exceeds the current Known Received Count (RFC 9204 §2.1.2). Counts
    /// *streams*, not sections: a stream with three blocking sections contributes one.
    ///
    /// The returned value is a snapshot — by the time the caller acts on it, an
    /// acknowledgement or Insert Count Increment may have reduced the true count.
    pub(in crate::headers) fn currently_blocked_streams(&self) -> usize {
        self.state.lock().unwrap().currently_blocked_streams()
    }

    /// Whether `stream_id` currently has at least one outstanding section with
    /// `required_insert_count > known_received_count`. A `true` result means the encoder
    /// may emit additional blocking references on `stream_id` without consuming a new
    /// blocked-streams budget slot (the stream is already counted).
    pub(in crate::headers) fn is_stream_blocking(&self, stream_id: u64) -> bool {
        self.state.lock().unwrap().is_stream_blocking(stream_id)
    }

    /// Ask whether the encoder is allowed to transition `stream_id` into the blocked set
    /// (RFC 9204 §2.1.2). Returns `true` if either `stream_id` is already blocking (free —
    /// no new slot consumed), or a free slot is available under the peer's
    /// `SETTINGS_QPACK_BLOCKED_STREAMS`.
    ///
    /// Intended to be called at most once per outbound section, at the point where
    /// `encode()` is about to commit to emitting its first reference that would push the
    /// section's RIC above KRC. Subsequent references within the same section inherit the
    /// commitment and do not need to re-query.
    pub(in crate::headers) fn can_block_another_stream(&self, stream_id: u64) -> bool {
        let state = self.state.lock().unwrap();
        if state.is_stream_blocking(stream_id) {
            return true;
        }
        state.currently_blocked_streams() < state.max_blocked_streams
    }

    /// Record a header section just emitted on `stream_id`, so that its pinned entries are
    /// protected from eviction and its Required Insert Count can advance
    /// `known_received_count` when the peer acknowledges it (RFC 9204 §2.1.1, §4.4.1).
    ///
    /// The caller is responsible for ensuring that any blocked-streams budget check has
    /// already been made; this method unconditionally records the section.
    pub(in crate::headers) fn register_outstanding_section(
        &self,
        stream_id: u64,
        refs: SectionRefs,
    ) {
        self.state
            .lock()
            .unwrap()
            .outstanding_sections
            .entry(stream_id)
            .or_default()
            .push_back(refs);
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

    /// Record a Section Acknowledgement received from the peer's decoder stream
    /// (RFC 9204 §4.4.1). Pops the oldest outstanding section for `stream_id` and, if its
    /// `required_insert_count` exceeds the current known-received count, advances the
    /// latter. Returns an error if no section is outstanding for this stream (protocol
    /// error by the peer).
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

    /// Record a Stream Cancellation received from the peer's decoder stream
    /// (RFC 9204 §4.4.2). Drops all outstanding sections for `stream_id` without advancing
    /// known-received.
    pub(in crate::headers) fn on_stream_cancel(&self, stream_id: u64) {
        let mut state = self.state.lock().unwrap();
        state.outstanding_sections.remove(&stream_id);
        drop(state);
        self.event.notify(usize::MAX);
    }

    /// Record an Insert Count Increment received from the peer's decoder stream
    /// (RFC 9204 §4.4.3). Advances known-received by `increment`. Returns an error if this
    /// would exceed `insert_count` (protocol error by the peer).
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
    pub(in crate::headers) fn fail(&self, code: H3ErrorCode) {
        self.state.lock().unwrap().failed = Some(code);
        self.event.notify(usize::MAX);
    }

    /// Returns `Some(code)` if the table has been marked failed.
    pub(in crate::headers) fn failed(&self) -> Option<H3ErrorCode> {
        self.state.lock().unwrap().failed
    }
}
