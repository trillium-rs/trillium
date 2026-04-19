use super::{encode::encode_required_insert_count, *};
use crate::{
    KnownHeaderName,
    headers::qpack::{
        decoder_dynamic_table::DecoderDynamicTable,
        instruction::encoder::{EncoderInstruction, parse},
        static_table::PseudoHeaderName,
    },
};
use futures_lite::future::block_on;

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
/// Mnemonic indexing is **off** by default in these helpers so that phase-2 mechanism
/// tests (eager insert, warming insert, blocked-stream paths) are not gated on the
/// predictor latching. Phase-3 tests that want predictor-driven behavior use
/// [`new_table_with_mnemonic`].
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
    new_table_configured(max_capacity, max_blocked_streams, false)
}

/// Phase-3 variant — mnemonic indexing enabled. Used by tests that exercise the predictor's
/// effect on indexing decisions. A non-zero blocked-streams budget is the default so the
/// insert-then-reference path is exercised on a predictor hit; tests that want warming-
/// insert behavior instead can call [`new_table_configured`] directly with
/// `max_blocked_streams = 0`.
fn new_table_with_mnemonic(max_capacity: u64) -> EncoderDynamicTable {
    new_table_configured(max_capacity, 100, true)
}

fn new_table_configured(
    max_capacity: u64,
    max_blocked_streams: u64,
    mnemonic_indexing: bool,
) -> EncoderDynamicTable {
    let table = EncoderDynamicTable::default();
    table.initialize_from_peer_settings(
        max_capacity as usize,
        H3Settings::default()
            .with_qpack_max_table_capacity(max_capacity)
            .with_qpack_blocked_streams(max_blocked_streams),
        mnemonic_indexing,
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
mod draining;
mod dup_draining;
mod encode_blocked;
mod encode_dynamic;
mod encode_refs;
mod encode_static;
mod insert;
mod mnemonic;
mod name_only;
mod pinning;
mod reverse_index;
