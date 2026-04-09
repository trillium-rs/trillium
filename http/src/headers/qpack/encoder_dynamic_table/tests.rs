use super::{encode::encode_required_insert_count, *};
use crate::{
    HttpContext, KnownHeaderName,
    h3::H3Error,
    headers::qpack::{
        ConnectionAccumulator,
        decoder_dynamic_table::DecoderDynamicTable,
        entry_name::QpackEntryName,
        instruction::encoder::{EncoderInstruction, parse},
        static_table::PseudoHeaderName,
    },
};
use futures_lite::future::block_on;

/// Test-only inspection and mutation helpers on [`EncoderDynamicTable`]. Production code
/// drives the table through the `encode` / `initialize_from_peer_settings` surface and the
/// ack/cancel/increment handlers; these wrappers exist so unit tests can exercise the
/// state-layer primitives directly (insert, `set_capacity`, reverse-index lookup, blocked-
/// streams accounting) without having to stand up a full field-section flow.
impl EncoderDynamicTable {
    /// The current `insert_count` — total entries ever inserted.
    pub(in crate::headers) fn insert_count(&self) -> u64 {
        self.state.lock().unwrap().insert_count
    }

    /// The current known-received count.
    pub(in crate::headers) fn known_received_count(&self) -> u64 {
        self.state.lock().unwrap().known_received_count
    }

    /// The number of live entries currently in the table.
    pub(in crate::headers) fn entry_count(&self) -> usize {
        self.state.lock().unwrap().entries.len()
    }

    /// Current total bytes used in the dynamic table. Diagnostic accessor for the
    /// per-group state snapshot in the corpus ASCII dump.
    pub(in crate::headers) fn current_size(&self) -> usize {
        self.state.lock().unwrap().current_size
    }

    /// Current maximum capacity (bytes). Diagnostic accessor for the corpus ASCII dump.
    pub(in crate::headers) fn capacity(&self) -> usize {
        self.state.lock().unwrap().capacity
    }

    /// Insert `(name, value)` into the dynamic table, picking the §3.2 wire format inside
    /// [`TableState::insert`]. Returns the absolute index of the freshly-inserted entry.
    pub(in crate::headers) fn insert(
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
    pub(in crate::headers) fn set_capacity(&self, new_capacity: usize) -> Result<(), H3Error> {
        let mut state = self.state.lock().unwrap();
        state.set_capacity(new_capacity)?;
        drop(state);
        self.event.notify(usize::MAX);
        Ok(())
    }

    /// Look up a dynamic-table entry whose name and value both match. Returns the absolute
    /// index of the latest such entry, or `None` if no live entry has this `(name, value)`.
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
    /// Insert Count exceeds the current Known Received Count (RFC 9204 §2.1.2).
    pub(in crate::headers) fn currently_blocked_streams(&self) -> usize {
        self.state.lock().unwrap().currently_blocked_streams()
    }

    /// Whether `stream_id` currently has at least one outstanding section with
    /// `required_insert_count > known_received_count`.
    pub(in crate::headers) fn is_stream_blocking(&self, stream_id: u64) -> bool {
        self.state.lock().unwrap().is_stream_blocking(stream_id)
    }

    /// Whether the encoder is allowed to transition `stream_id` into the blocked set
    /// (RFC 9204 §2.1.2): either `stream_id` is already blocking (free — no new slot
    /// consumed), or a free slot is available under the peer's
    /// `SETTINGS_QPACK_BLOCKED_STREAMS`.
    pub(in crate::headers) fn can_block_another_stream(&self, stream_id: u64) -> bool {
        let state = self.state.lock().unwrap();
        if state.is_stream_blocking(stream_id) {
            return true;
        }
        state.currently_blocked_streams() < state.max_blocked_streams
    }

    /// Record an outstanding header section pinning a set of entries, without actually
    /// emitting the corresponding field-section bytes. Used by tests that want to drive
    /// eviction-floor / blocked-streams bookkeeping directly.
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
}

// Test helpers — kept small and explicit.

/// Construct a [`QpackEntryName`] from a string. Handles known headers (`"accept-encoding"`),
/// pseudo-headers (`":path"`), and unknown headers (`"x-custom"`) uniformly.
fn qen(s: &str) -> QpackEntryName<'static> {
    QpackEntryName::try_from(s.as_bytes().to_vec()).unwrap()
}

/// Construct an owned [`FieldLineValue`] from a static string — the common test shape.
fn fv(s: &'static str) -> FieldLineValue<'static> {
    FieldLineValue::Static(s.as_bytes())
}

/// Construct a [`FieldLineValue::Owned`] from a byte vector. Used for tests that need
/// dynamically-sized values (e.g. the oversize case).
fn fvo(v: Vec<u8>) -> FieldLineValue<'static> {
    FieldLineValue::Owned(v)
}

/// Construct a fresh encoder table at the given capacity and initialize it from peer
/// settings.
///
/// The initialization `SetDynamicTableCapacity` is left in `pending_ops` as it would be on
/// the wire — tests that drain for variant assertions see it as the leading op, and
/// [`apply_ops_to_decoder`] consumes it naturally to prime the decoder's capacity.
fn new_table(max_capacity: u64) -> EncoderDynamicTable {
    new_table_with_blocked_streams(max_capacity, 0)
}

fn new_table_with_blocked_streams(
    max_capacity: u64,
    max_blocked_streams: u64,
) -> EncoderDynamicTable {
    let context = HttpContext::default().with_config(
        crate::HttpConfig::default().with_h3_max_table_capacity(max_capacity as usize),
    );
    let table = EncoderDynamicTable::new(&context);
    table.initialize_from_peer_settings(
        H3Settings::default()
            .with_qpack_max_table_capacity(max_capacity)
            .with_qpack_blocked_streams(max_blocked_streams),
    );
    table
}

/// Drain the table's pending encoder-stream ops and parse them back into typed
/// [`EncoderInstruction`]s. Use this to assert which §3.2 wire format the single-entry-point
/// `insert` picker chose, and to recover decoded values with Huffman already resolved.
fn drain_instructions(table: &EncoderDynamicTable) -> Vec<EncoderInstruction> {
    let bytes: Vec<u8> = table.drain_pending_ops().into_iter().flatten().collect();
    parse_all(&bytes)
}

fn parse_all(bytes: &[u8]) -> Vec<EncoderInstruction> {
    let mut stream = bytes;
    let mut out = Vec::new();
    while let Some(instr) = block_on(parse(usize::MAX, &mut stream)).unwrap() {
        out.push(instr);
    }
    out
}

/// Apply the encoder's currently-pending ops to a matching decoder table and return it.
/// Used for end-to-end semantic roundtrip assertions: the encoder's choices should
/// reconstruct the same entries on the peer side regardless of which §3.2 wire format was
/// picked. The encoder's leading `SetDynamicTableCapacity` op primes the decoder — no
/// side-channel capacity call is needed, matching how this flows on the wire in production.
fn apply_ops_to_decoder(table: &EncoderDynamicTable, max_capacity: u64) -> DecoderDynamicTable {
    let bytes: Vec<u8> = table.drain_pending_ops().into_iter().flatten().collect();
    let decoder = DecoderDynamicTable::new(max_capacity as usize, 0);
    let mut stream = &bytes[..];
    block_on(decoder.run_reader(&mut stream)).unwrap();
    decoder
}

fn blocking_section(ric: u64) -> SectionRefs {
    SectionRefs {
        required_insert_count: ric,
        min_ref_abs_idx: None,
    }
}

mod budgets_and_capacity;
mod dup_drain;
mod encode_blocked;
mod encode_dynamic;
mod encode_refs;
mod encode_static;
mod insert;
mod pinning;
mod reverse_index;
