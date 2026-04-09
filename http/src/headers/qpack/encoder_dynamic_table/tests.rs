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

// ----------------------------------------------------------------------------
// Test helpers — kept small and explicit.
// ----------------------------------------------------------------------------

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
    let table = EncoderDynamicTable::default();
    table.initialize_from_peer_settings(
        max_capacity as usize,
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

// ----------------------------------------------------------------------------
// Blocked-streams accounting
// ----------------------------------------------------------------------------

#[test]
fn blocked_streams_empty_budget_available() {
    let table = new_table_with_blocked_streams(4096, 2);
    assert_eq!(table.currently_blocked_streams(), 0);
    assert!(!table.is_stream_blocking(1));
    assert!(table.can_block_another_stream(1));
}

#[test]
fn blocked_streams_non_blocking_section_does_not_count() {
    // RIC == 0 is a static-only section and never blocks, even when KRC is also 0.
    let table = new_table_with_blocked_streams(4096, 0);
    table.register_outstanding_section(1, blocking_section(0));
    assert_eq!(table.currently_blocked_streams(), 0);
    assert!(!table.is_stream_blocking(1));
}

#[test]
fn blocked_streams_counts_distinct_streams() {
    let table = new_table_with_blocked_streams(4096, 3);
    table.register_outstanding_section(1, blocking_section(5));
    table.register_outstanding_section(2, blocking_section(7));
    assert_eq!(table.currently_blocked_streams(), 2);
    assert!(table.is_stream_blocking(1));
    assert!(table.is_stream_blocking(2));
    assert!(!table.is_stream_blocking(3));
}

#[test]
fn blocked_streams_per_stream_dedup() {
    // Three sections on one stream — all blocking, but it's still one blocked stream.
    let table = new_table_with_blocked_streams(4096, 1);
    table.register_outstanding_section(1, blocking_section(3));
    table.register_outstanding_section(1, blocking_section(5));
    table.register_outstanding_section(1, blocking_section(7));
    assert_eq!(table.currently_blocked_streams(), 1);
    assert!(table.can_block_another_stream(1)); // already blocking, free
    assert!(!table.can_block_another_stream(2)); // would exceed budget of 1
}

#[test]
fn blocked_streams_budget_exhausted() {
    let table = new_table_with_blocked_streams(4096, 2);
    table.register_outstanding_section(1, blocking_section(5));
    table.register_outstanding_section(2, blocking_section(7));
    assert_eq!(table.currently_blocked_streams(), 2);
    assert!(table.can_block_another_stream(1)); // already blocking
    assert!(table.can_block_another_stream(2)); // already blocking
    assert!(!table.can_block_another_stream(3)); // new stream, budget full
}

// ----------------------------------------------------------------------------
// Required Insert Count encoding (§4.5.1.1)
// ----------------------------------------------------------------------------

#[test]
fn encode_required_insert_count_formula() {
    // ric == 0 always encodes as 0 regardless of capacity.
    assert_eq!(encode_required_insert_count(0, 0), 0);
    assert_eq!(encode_required_insert_count(0, 4096), 0);

    // max_capacity 32 → max_entries 1, full_range 2.
    assert_eq!(encode_required_insert_count(1, 32), 2); // (1 % 2) + 1
    assert_eq!(encode_required_insert_count(2, 32), 1); // (2 % 2) + 1
    assert_eq!(encode_required_insert_count(3, 32), 2);

    // max_capacity 100 → max_entries 3, full_range 6.
    assert_eq!(encode_required_insert_count(5, 100), 6); // (5 % 6) + 1
    assert_eq!(encode_required_insert_count(6, 100), 1); // (6 % 6) + 1
    assert_eq!(encode_required_insert_count(13, 100), 2); // (13 % 6) + 1

    // max_capacity 4096 → max_entries 128, full_range 256.
    assert_eq!(encode_required_insert_count(1, 4096), 2);
    assert_eq!(encode_required_insert_count(256, 4096), 1);
    assert_eq!(encode_required_insert_count(300, 4096), 45); // (300 % 256) + 1
}

// ----------------------------------------------------------------------------
// Set Dynamic Table Capacity (§3.2.1)
// ----------------------------------------------------------------------------

#[test]
fn set_capacity_emits_wire_instruction() {
    let table = new_table(4096);
    table.set_capacity(1024).unwrap();
    assert_eq!(
        drain_instructions(&table),
        vec![
            EncoderInstruction::SetCapacity(4096),
            EncoderInstruction::SetCapacity(1024),
        ]
    );
}

#[test]
fn set_capacity_rejects_above_max() {
    let table = new_table(4096);
    assert!(table.set_capacity(8192).is_err());
}

#[test]
fn drain_is_fifo() {
    // Initialization emitted SetCapacity(4096); two more to verify FIFO ordering on the wire.
    let table = new_table(4096);
    table.set_capacity(256).unwrap();
    table.set_capacity(1024).unwrap();
    assert_eq!(
        drain_instructions(&table),
        vec![
            EncoderInstruction::SetCapacity(4096),
            EncoderInstruction::SetCapacity(256),
            EncoderInstruction::SetCapacity(1024),
        ]
    );
}

// ----------------------------------------------------------------------------
// Insert — wire-format selection (§3.2.2–§3.2.4)
//
// The encoder's single `insert(name, value)` entry point picks the smallest §3.2 wire form
// based on current table state:
//   1. Duplicate         — (name, value) already lives in the table
//   2. Static name ref   — name has a slot in the QPACK static table
//   3. Dynamic name ref  — a live dynamic entry with this name already exists
//   4. Literal name      — otherwise
// Each test below sets up the state needed to force a specific branch and then checks both
// the chosen wire format (via the parsed instruction) and the round-tripped decoder view.
// ----------------------------------------------------------------------------

#[test]
fn insert_literal_name_for_unknown_header_without_prior_entry() {
    let table = new_table(4096);
    let abs = table.insert(qen("x-custom"), fv("hello")).unwrap();
    assert_eq!(abs, 0);
    assert_eq!(table.insert_count(), 1);

    assert_eq!(
        drain_instructions(&table),
        vec![
            EncoderInstruction::SetCapacity(4096),
            EncoderInstruction::InsertWithLiteralName {
                name: qen("x-custom"),
                value: b"hello".to_vec(),
            },
        ]
    );
}

#[test]
fn insert_roundtrips_through_decoder_as_literal_name() {
    let table = new_table(4096);
    table.insert(qen("x-custom"), fv("hello")).unwrap();
    let decoder = apply_ops_to_decoder(&table, 4096);
    assert_eq!(decoder.name_at_relative(0).unwrap().as_ref(), "x-custom");
}

#[test]
fn insert_rejects_entry_exceeding_capacity() {
    let table = new_table(64);
    // entry_size = name + value + 32; a 64-byte value forces entry_size > 64.
    let big = fvo(vec![b'x'; 64]);
    assert!(table.insert(qen("n"), big).is_err());
}

#[test]
fn literal_name_inserts_evict_oldest_when_unpinned() {
    // Capacity 70 fits exactly two entries of size 34 (1 + 1 + 32). The third insert must
    // evict the oldest. None of these names have static slots, so each insert chooses the
    // literal-name form.
    let table = new_table(70);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    assert_eq!(table.entry_count(), 2);

    table.insert(qen("c"), fv("3")).unwrap();
    assert_eq!(table.insert_count(), 3);
    assert_eq!(table.entry_count(), 2);

    // Oldest ("a") is gone; newest two are "b" and "c".
    assert_eq!(table.find_name_match(&qen("a")), None);
    assert!(table.find_name_match(&qen("b")).is_some());
    assert!(table.find_name_match(&qen("c")).is_some());
}

#[test]
fn insert_known_header_with_static_slot_chooses_static_name_ref() {
    // `accept-encoding` has static table entries; a custom value triggers Insert With Name
    // Reference (T=1) rather than a literal name.
    let table = new_table(4096);
    table
        .insert(KnownHeaderName::AcceptEncoding.into(), fv("identity"))
        .unwrap();

    let instrs = drain_instructions(&table);
    assert!(matches!(
        instrs.as_slice(),
        [
            EncoderInstruction::SetCapacity(_),
            EncoderInstruction::InsertWithStaticNameRef { value, .. },
        ] if value == b"identity"
    ));
}

#[test]
fn insert_pseudo_header_with_static_slot_chooses_static_name_ref() {
    // `:path` has static slot 1 (`:path: /`). A different path value still uses the static
    // name reference.
    let table = new_table(4096);
    table.insert(qen(":path"), fv("/api/users")).unwrap();

    let instrs = drain_instructions(&table);
    assert!(matches!(
        instrs.as_slice(),
        [
            EncoderInstruction::SetCapacity(_),
            EncoderInstruction::InsertWithStaticNameRef {
                name_index: 1,
                value,
            },
        ] if value == b"/api/users"
    ));
}

#[test]
fn insert_pseudo_header_without_static_slot_uses_literal_name() {
    // `:protocol` has no static slot; the first insert lands as a literal name.
    let table = new_table(4096);
    table.insert(qen(":protocol"), fv("webtransport")).unwrap();

    let instrs = drain_instructions(&table);
    assert!(matches!(
        &instrs[..],
        [
            EncoderInstruction::SetCapacity(_),
            EncoderInstruction::InsertWithLiteralName { name, value },
        ] if name == &qen(":protocol") && value == b"webtransport"
    ));
}

#[test]
fn insert_second_value_for_unknown_name_chooses_dynamic_name_ref() {
    // `x-custom` has no static slot, so the second insert with the same name but a different
    // value falls through to dynamic-name-ref (T=0).
    let table = new_table(4096);
    table.insert(qen("x-custom"), fv("first")).unwrap();
    let abs = table.insert(qen("x-custom"), fv("second")).unwrap();
    assert_eq!(abs, 1);

    let instrs = drain_instructions(&table);
    assert_eq!(instrs.len(), 3);
    assert!(matches!(instrs[0], EncoderInstruction::SetCapacity(_)));
    assert!(matches!(
        instrs[1],
        EncoderInstruction::InsertWithLiteralName { .. }
    ));
    assert!(matches!(
        &instrs[2],
        EncoderInstruction::InsertWithDynamicNameRef { relative_index: 0, value }
            if value == b"second"
    ));
}

#[test]
fn dynamic_name_ref_insert_preserves_source_when_capacity_tight() {
    // Capacity 34 fits exactly one entry of size 34. The first entry has no static slot, so
    // the second insert (same name, different value) takes the dynamic-name-ref path — which
    // has to keep the source entry alive across eviction. No room → the insert must error.
    let table = new_table(34);
    table.insert(qen("a"), fv("1")).unwrap();
    assert!(table.insert(qen("a"), fv("2")).is_err());
    // Source entry is still there.
    assert_eq!(table.entry_count(), 1);
    assert_eq!(table.find_full_match(&qen("a"), b"1"), Some(0));
}

#[test]
fn second_insert_of_same_name_value_chooses_duplicate() {
    let table = new_table(4096);
    table.insert(qen("x-custom"), fv("hello")).unwrap();
    let second = table.insert(qen("x-custom"), fv("hello")).unwrap();
    assert_eq!(second, 1);
    assert_eq!(table.insert_count(), 2);
    assert_eq!(table.entry_count(), 2);

    let instrs = drain_instructions(&table);
    assert_eq!(instrs.len(), 3);
    assert!(matches!(
        instrs[2],
        EncoderInstruction::Duplicate { relative_index: 0 }
    ));
}

#[test]
fn duplicate_is_chosen_even_when_static_name_ref_is_available() {
    // `content-type` has a static name slot, but since the (name, value) pair already exists
    // in the dynamic table, Duplicate is strictly cheaper on the wire (no value bytes).
    let table = new_table(4096);
    table
        .insert(KnownHeaderName::ContentType.into(), fv("application/json"))
        .unwrap();
    table
        .insert(KnownHeaderName::ContentType.into(), fv("application/json"))
        .unwrap();

    let instrs = drain_instructions(&table);
    assert!(matches!(
        instrs[1],
        EncoderInstruction::InsertWithStaticNameRef { .. }
    ));
    assert!(matches!(
        instrs[2],
        EncoderInstruction::Duplicate { relative_index: 0 }
    ));
}

#[test]
fn duplicate_picks_matching_entry_across_multiple_names() {
    // Picker must match the full (name, value) pair, not just the name. Even though `a:1` at
    // abs 0 and `a:1` could in principle be duplicated, the intermediate `b:1` must not be.
    let table = new_table(4096);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("1")).unwrap();
    let third = table.insert(qen("a"), fv("1")).unwrap();
    assert_eq!(third, 2);

    let instrs = drain_instructions(&table);
    assert_eq!(instrs.len(), 4);
    // Last op must be a Duplicate. RFC §3.2.4 relative index = insert_count - 1 - abs_idx;
    // at the moment the duplicate is encoded, insert_count = 2 and the source abs_idx = 0,
    // so relative_index = 1.
    assert_eq!(
        instrs[3],
        EncoderInstruction::Duplicate { relative_index: 1 }
    );
}

#[test]
fn duplicate_source_remains_live_after_duplicate() {
    let table = new_table(4096);
    let first = table.insert(qen("x-custom"), fv("hello")).unwrap();
    let second = table.insert(qen("x-custom"), fv("hello")).unwrap();
    assert_eq!((first, second), (0, 1));
    assert_eq!(table.entry_count(), 2);

    // Both abs indices resolve, and the full-match lookup returns the newer one.
    assert_eq!(
        table.find_full_match(&qen("x-custom"), b"hello"),
        Some(second)
    );
}

#[test]
fn duplicate_fails_when_capacity_only_fits_one_entry() {
    // Capacity 34 fits exactly one entry of size 34. Duplicating the sole entry would need
    // to evict it first — but it's also the source we're copying from, so eviction is
    // blocked by the preserve-source floor.
    let table = new_table(34);
    table.insert(qen("a"), fv("1")).unwrap();
    assert!(table.insert(qen("a"), fv("1")).is_err());
    assert_eq!(table.entry_count(), 1);
}

#[test]
fn duplicate_succeeds_when_capacity_fits_two_entries() {
    // Capacity 68 fits two entries of size 34. Duplicating the sole entry succeeds.
    let table = new_table(68);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("a"), fv("1")).unwrap();
    assert_eq!(table.entry_count(), 2);

    let instrs = drain_instructions(&table);
    assert!(matches!(
        instrs[2],
        EncoderInstruction::Duplicate { relative_index: 0 }
    ));
}

// ----------------------------------------------------------------------------
// Roundtrip sanity — the decoder reconstructs the same (name, value) regardless of which
// §3.2 wire format the encoder picked.
// ----------------------------------------------------------------------------

#[test]
fn roundtrip_static_name_ref_insert() {
    let table = new_table(4096);
    table
        .insert(KnownHeaderName::AcceptEncoding.into(), fv("identity"))
        .unwrap();
    let decoder = apply_ops_to_decoder(&table, 4096);
    assert_eq!(
        decoder.name_at_relative(0).unwrap().as_ref(),
        "accept-encoding"
    );
}

#[test]
fn roundtrip_dynamic_name_ref_insert() {
    let table = new_table(4096);
    table.insert(qen("x-custom"), fv("first")).unwrap();
    table.insert(qen("x-custom"), fv("second")).unwrap();
    let decoder = apply_ops_to_decoder(&table, 4096);
    assert_eq!(decoder.name_at_relative(0).unwrap().as_ref(), "x-custom");
    assert_eq!(decoder.name_at_relative(1).unwrap().as_ref(), "x-custom");
}

#[test]
fn roundtrip_duplicate() {
    let table = new_table(4096);
    table.insert(qen("x-custom"), fv("hello")).unwrap();
    table.insert(qen("x-custom"), fv("hello")).unwrap();
    let decoder = apply_ops_to_decoder(&table, 4096);
    assert_eq!(decoder.name_at_relative(0).unwrap().as_ref(), "x-custom");
    assert_eq!(decoder.name_at_relative(1).unwrap().as_ref(), "x-custom");
}

#[test]
fn roundtrip_pseudo_header_without_static_slot() {
    // Covers the literal-name path for a pseudo-header (`:protocol`).
    let table = new_table(4096);
    table
        .insert(
            QpackEntryName::Pseudo(PseudoHeaderName::Protocol),
            fv("webtransport"),
        )
        .unwrap();
    let decoder = apply_ops_to_decoder(&table, 4096);
    assert_eq!(decoder.name_at_relative(0).unwrap().as_ref(), ":protocol");
}

// ----------------------------------------------------------------------------
// Pinning and ack/cancel/increment bookkeeping
// ----------------------------------------------------------------------------

#[test]
fn pinned_entry_blocks_eviction() {
    // Capacity 70 fits two entries; the third insert would normally evict the oldest. Pin
    // abs index 0 and the insert must error.
    let table = new_table(70);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    table.register_outstanding_section(
        4,
        SectionRefs {
            required_insert_count: 1,
            min_ref_abs_idx: Some(0),
        },
    );
    assert!(table.insert(qen("c"), fv("3")).is_err());
    assert_eq!(table.entry_count(), 2);
}

#[test]
fn section_ack_advances_known_received() {
    let table = new_table(4096);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    table.register_outstanding_section(
        4,
        SectionRefs {
            required_insert_count: 2,
            min_ref_abs_idx: Some(0),
        },
    );
    table.on_section_ack(4).unwrap();
    assert_eq!(table.known_received_count(), 2);
}

#[test]
fn section_ack_without_outstanding_errors() {
    let table = EncoderDynamicTable::default();
    assert!(table.on_section_ack(4).is_err());
}

#[test]
fn stream_cancel_drops_sections_without_advancing() {
    let table = EncoderDynamicTable::default();
    table.register_outstanding_section(
        4,
        SectionRefs {
            required_insert_count: 3,
            min_ref_abs_idx: Some(0),
        },
    );
    table.on_stream_cancel(4);
    assert_eq!(table.known_received_count(), 0);
    assert!(table.state.lock().unwrap().outstanding_sections.is_empty());
}

#[test]
fn insert_count_increment_advances_and_bounds() {
    let table = new_table(4096);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    table.on_insert_count_increment(1).unwrap();
    assert_eq!(table.known_received_count(), 1);
    table.on_insert_count_increment(1).unwrap();
    assert_eq!(table.known_received_count(), 2);
    // Cannot advance past insert_count.
    assert!(table.on_insert_count_increment(1).is_err());
}

#[test]
fn insert_count_increment_rejects_zero() {
    let table = EncoderDynamicTable::default();
    assert!(table.on_insert_count_increment(0).is_err());
}

#[test]
fn fail_sets_failed_state() {
    let table = EncoderDynamicTable::default();
    table.fail(H3ErrorCode::QpackEncoderStreamError);
    assert_eq!(table.failed(), Some(H3ErrorCode::QpackEncoderStreamError));
}

// ----------------------------------------------------------------------------
// Reverse index (by_name → by_value / latest_any)
// ----------------------------------------------------------------------------

#[test]
fn reverse_index_basic_lookup() {
    let table = new_table(4096);
    let abs = table.insert(qen("x-custom"), fv("hello")).unwrap();
    let name = qen("x-custom");
    assert_eq!(table.find_full_match(&name, b"hello"), Some(abs));
    assert_eq!(table.find_name_match(&name), Some(abs));
    assert_eq!(table.find_full_match(&name, b"other"), None);
    assert_eq!(table.find_name_match(&qen("x-missing")), None);
}

#[test]
fn reverse_index_duplicate_updates_to_newer_abs_idx() {
    let table = new_table(4096);
    table.insert(qen("x-custom"), fv("v")).unwrap();
    let second = table.insert(qen("x-custom"), fv("v")).unwrap();
    let name = qen("x-custom");
    // Both full-match and name-match should report the newer (higher) abs_idx.
    assert_eq!(table.find_full_match(&name, b"v"), Some(second));
    assert_eq!(table.find_name_match(&name), Some(second));
}

#[test]
fn reverse_index_staleness_preserves_newer_after_eviction() {
    // Capacity 68 fits exactly two entries of size 34. Insert (a,1), Duplicate it → two
    // entries with the same (name, value). Inserting (c,3) evicts the oldest (`a` at abs 0);
    // the per-value slot in the reverse index must NOT be cleared because a newer entry
    // still carries that value.
    let table = new_table(68);
    let first = table.insert(qen("a"), fv("1")).unwrap();
    let second = table.insert(qen("a"), fv("1")).unwrap(); // Duplicate wire form
    assert_ne!(first, second);
    table.insert(qen("c"), fv("3")).unwrap();
    assert_eq!(table.entry_count(), 2);

    let name_a = qen("a");
    assert_eq!(table.find_full_match(&name_a, b"1"), Some(second));
    assert_eq!(table.find_name_match(&name_a), Some(second));
}

#[test]
fn reverse_index_removes_name_when_all_values_evicted() {
    // Capacity 34 fits exactly one entry of size 34.
    let table = new_table(34);
    table.insert(qen("a"), fv("1")).unwrap();
    let b_abs = table.insert(qen("b"), fv("2")).unwrap();
    assert_eq!(table.entry_count(), 1);
    let name_a = qen("a");
    let name_b = qen("b");
    // "a" is fully evicted — neither lookup should find it.
    assert_eq!(table.find_full_match(&name_a, b"1"), None);
    assert_eq!(table.find_name_match(&name_a), None);
    // "b" is still live.
    assert_eq!(table.find_full_match(&name_b, b"2"), Some(b_abs));
    assert_eq!(table.find_name_match(&name_b), Some(b_abs));
}

// ----------------------------------------------------------------------------
// encode path tests — these exercise `EncoderDynamicTable::encode` directly so tests can
// pre-seed the dynamic table, adjust blocked-streams budget, and inspect
// `outstanding_sections`. End-to-end roundtrips through `H3Connection` live in
// `http/src/headers/qpack/tests.rs`.
// ----------------------------------------------------------------------------

mod encode_tests {
    use super::*;
    use crate::{
        Headers, KnownHeaderName, Method, Status,
        headers::qpack::{
            FieldSection, PseudoHeaders,
            decoder_dynamic_table::DecoderDynamicTable,
            instruction::field_section::{FieldLineInstruction, FieldSectionPrefix},
        },
    };
    use std::borrow::Cow;

    /// Parse an encoded field section into its typed prefix and the sequence of typed
    /// field-line instructions. Panics on parser error — tests shouldn't be exercising
    /// parse-error paths.
    #[track_caller]
    fn parse_section(bytes: &[u8]) -> (FieldSectionPrefix, Vec<FieldLineInstruction<'_>>) {
        let (prefix, mut rest) = FieldSectionPrefix::parse(bytes).expect("prefix parses");
        let mut lines = Vec::new();
        while !rest.is_empty() {
            let (instr, tail) = FieldLineInstruction::parse(rest).expect("field line parses");
            lines.push(instr);
            rest = tail;
        }
        (prefix, lines)
    }

    fn encode(
        table: &EncoderDynamicTable,
        pseudo: PseudoHeaders<'_>,
        headers: &Headers,
        stream_id: u64,
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        let field_section = FieldSection::new(pseudo, headers);
        table.encode(&field_section, &mut buf, stream_id);
        buf
    }

    /// Encode through `encoder`, drain encoder-stream ops into `decoder`, then decode the
    /// resulting field section through `decoder`. Panics on any error.
    #[track_caller]
    fn roundtrip(
        encoder: &EncoderDynamicTable,
        decoder: &DecoderDynamicTable,
        pseudo: PseudoHeaders<'_>,
        headers: &Headers,
        stream_id: u64,
    ) -> FieldSection<'static> {
        let bytes = encode(encoder, pseudo, headers, stream_id);
        let enc_ops: Vec<u8> = encoder.drain_pending_ops().into_iter().flatten().collect();
        let mut enc_stream = &enc_ops[..];
        block_on(decoder.run_reader(&mut enc_stream)).unwrap();
        block_on(decoder.decode(&bytes, stream_id)).unwrap()
    }

    fn outstanding(table: &EncoderDynamicTable, stream_id: u64) -> Vec<SectionRefs> {
        table
            .state
            .lock()
            .unwrap()
            .outstanding_sections
            .get(&stream_id)
            .cloned()
            .map(|d| d.into_iter().collect())
            .unwrap_or_default()
    }

    // ---- section prefix ----

    #[test]
    fn empty_field_section_emits_zero_prefix() {
        let table = new_table(4096);
        let bytes = encode(&table, PseudoHeaders::default(), &Headers::new(), 1);
        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        assert!(lines.is_empty());
        assert!(outstanding(&table, 1).is_empty());
    }

    #[test]
    fn static_only_section_has_zero_prefix_and_registers_nothing() {
        let table = new_table(4096);
        let bytes = encode(
            &table,
            PseudoHeaders {
                method: Some(Method::Get),
                path: Some(Cow::Borrowed("/")),
                scheme: Some(Cow::Borrowed("https")),
                ..Default::default()
            },
            &Headers::new(),
            1,
        );
        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        assert!(
            lines
                .iter()
                .all(|l| matches!(l, FieldLineInstruction::IndexedStatic { .. })),
            "expected only static full matches, got {lines:?}",
        );
        assert!(outstanding(&table, 1).is_empty());
    }

    // ---- static full match ----

    #[test]
    fn static_full_match_method_get() {
        let table = new_table(4096);
        let bytes = encode(
            &table,
            PseudoHeaders {
                method: Some(Method::Get),
                ..Default::default()
            },
            &Headers::new(),
            1,
        );
        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        // Static table index 17 = `:method GET` (RFC 9204 Appendix A).
        assert_eq!(
            lines,
            vec![FieldLineInstruction::IndexedStatic { index: 17 }]
        );
    }

    #[test]
    fn static_full_match_status_200() {
        let table = new_table(4096);
        let bytes = encode(
            &table,
            PseudoHeaders {
                status: Some(Status::Ok),
                ..Default::default()
            },
            &Headers::new(),
            1,
        );
        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        // Static table index 25 = `:status 200`.
        assert_eq!(
            lines,
            vec![FieldLineInstruction::IndexedStatic { index: 25 }]
        );
    }

    #[test]
    fn static_full_match_path_slash_and_scheme_https() {
        let table = new_table(4096);
        let bytes = encode(
            &table,
            PseudoHeaders {
                path: Some(Cow::Borrowed("/")),
                scheme: Some(Cow::Borrowed("https")),
                ..Default::default()
            },
            &Headers::new(),
            1,
        );
        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        // Static table: 1 = `:path /`, 23 = `:scheme https`.
        assert_eq!(
            lines,
            vec![
                FieldLineInstruction::IndexedStatic { index: 1 },
                FieldLineInstruction::IndexedStatic { index: 23 },
            ]
        );
    }

    // ---- static name reference ----

    #[test]
    fn static_name_ref_unknown_method_patch() {
        let table = new_table(4096);
        let bytes = encode(
            &table,
            PseudoHeaders {
                method: Some(Method::Patch),
                ..Default::default()
            },
            &Headers::new(),
            1,
        );
        let decoder = DecoderDynamicTable::new(4096, 0);
        let decoded = block_on(decoder.decode(&bytes, 1)).unwrap();
        let (pseudo, _) = decoded.into_parts();
        assert_eq!(pseudo.method(), Some(Method::Patch));
    }

    #[test]
    fn static_name_ref_content_type_custom_value() {
        let table = new_table(4096);
        let mut headers = Headers::new();
        headers.insert(KnownHeaderName::ContentType, "application/xml");
        let bytes = encode(&table, PseudoHeaders::default(), &headers, 1);

        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        let [
            FieldLineInstruction::LiteralStaticNameRef {
                value,
                never_indexed: false,
                ..
            },
        ] = lines.as_slice()
        else {
            panic!("expected a single literal with static name ref, got {lines:?}");
        };
        assert_eq!(value.as_bytes(), b"application/xml");
        assert!(outstanding(&table, 1).is_empty());
    }

    // ---- literal with literal name ----

    #[test]
    fn literal_literal_name_unknown_header() {
        let table = new_table(4096);
        let mut headers = Headers::new();
        headers.insert("x-custom", "hello");
        let bytes = encode(&table, PseudoHeaders::default(), &headers, 1);

        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        let [
            FieldLineInstruction::LiteralLiteralName {
                name,
                value,
                never_indexed: false,
            },
        ] = lines.as_slice()
        else {
            panic!("expected a single literal-with-literal-name, got {lines:?}");
        };
        assert_eq!(name.as_bytes(), b"x-custom");
        assert_eq!(value.as_bytes(), b"hello");
        assert!(outstanding(&table, 1).is_empty());

        let decoder = DecoderDynamicTable::new(4096, 0);
        let decoded = block_on(decoder.decode(&bytes, 1)).unwrap();
        let (_, decoded_headers) = decoded.into_parts();
        assert_eq!(decoded_headers.get_str("x-custom"), Some("hello"));
    }

    #[test]
    fn literal_literal_name_protocol_with_no_dynamic_entry() {
        let table = new_table(4096);
        let bytes = encode(
            &table,
            PseudoHeaders {
                protocol: Some(Cow::Borrowed("webtransport")),
                ..Default::default()
            },
            &Headers::new(),
            1,
        );
        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        let [
            FieldLineInstruction::LiteralLiteralName {
                name,
                value,
                never_indexed: false,
            },
        ] = lines.as_slice()
        else {
            panic!("expected a single literal-with-literal-name, got {lines:?}");
        };
        assert_eq!(name.as_bytes(), b":protocol");
        assert_eq!(value.as_bytes(), b"webtransport");

        let decoder = DecoderDynamicTable::new(4096, 0);
        let decoded = block_on(decoder.decode(&bytes, 1)).unwrap();
        let (pseudo, _) = decoded.into_parts();
        assert_eq!(pseudo.protocol(), Some("webtransport"));
    }

    // ---- dynamic full match ----

    #[test]
    fn dynamic_full_match_regular_header() {
        let encoder = new_table(4096);
        let abs = encoder.insert(qen("x-custom"), fv("hello")).unwrap();
        assert_eq!(abs, 0);
        encoder.on_insert_count_increment(1).unwrap();

        let mut headers = Headers::new();
        headers.insert("x-custom", "hello");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let (prefix, lines) = parse_section(&bytes);
        // RIC=1 → encoded_ric = (1 % 256) + 1 = 2; base = RIC, so delta_base = 0.
        assert_eq!(
            prefix,
            FieldSectionPrefix {
                encoded_required_insert_count: 2,
                base_is_negative: false,
                delta_base: 0,
            }
        );
        assert_eq!(
            lines,
            vec![FieldLineInstruction::IndexedDynamic { relative_index: 0 }]
        );

        let os = outstanding(&encoder, 1);
        assert_eq!(os.len(), 1);
        assert_eq!(os[0].required_insert_count, 1);
        assert_eq!(os[0].min_ref_abs_idx, Some(0));
    }

    #[test]
    fn dynamic_full_match_preferred_over_static() {
        let encoder = new_table(4096);
        encoder
            .insert(KnownHeaderName::ContentType.into(), fv("application/json"))
            .unwrap();
        encoder.on_insert_count_increment(1).unwrap();

        let mut headers = Headers::new();
        headers.insert(KnownHeaderName::ContentType, "application/json");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let (prefix, lines) = parse_section(&bytes);
        assert_ne!(prefix.encoded_required_insert_count, 0);
        assert_eq!(
            lines,
            vec![FieldLineInstruction::IndexedDynamic { relative_index: 0 }]
        );
    }

    #[test]
    fn dynamic_full_match_pseudo_method() {
        // `:method PATCH` has no static full match (only name ref to 15). Seed the dynamic
        // table so a later encode finds a full dynamic match.
        let encoder = new_table(4096);
        let abs = encoder
            .insert(
                QpackEntryName::Pseudo(PseudoHeaderName::Method),
                fv("PATCH"),
            )
            .unwrap();
        assert_eq!(abs, 0);
        encoder.on_insert_count_increment(1).unwrap();

        let bytes = encode(
            &encoder,
            PseudoHeaders {
                method: Some(Method::Patch),
                ..Default::default()
            },
            &Headers::new(),
            1,
        );
        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(
            prefix,
            FieldSectionPrefix {
                encoded_required_insert_count: 2,
                base_is_negative: false,
                delta_base: 0,
            }
        );
        assert_eq!(
            lines,
            vec![FieldLineInstruction::IndexedDynamic { relative_index: 0 }]
        );
    }

    #[test]
    fn dynamic_full_match_protocol() {
        let encoder = new_table(4096);
        encoder
            .insert(
                QpackEntryName::Pseudo(PseudoHeaderName::Protocol),
                fv("webtransport"),
            )
            .unwrap();
        encoder.on_insert_count_increment(1).unwrap();

        let bytes = encode(
            &encoder,
            PseudoHeaders {
                protocol: Some(Cow::Borrowed("webtransport")),
                ..Default::default()
            },
            &Headers::new(),
            1,
        );
        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(
            prefix,
            FieldSectionPrefix {
                encoded_required_insert_count: 2,
                base_is_negative: false,
                delta_base: 0,
            }
        );
        assert_eq!(
            lines,
            vec![FieldLineInstruction::IndexedDynamic { relative_index: 0 }]
        );

        let os = outstanding(&encoder, 1);
        assert_eq!(os.len(), 1);
        assert_eq!(os[0].required_insert_count, 1);
        assert_eq!(os[0].min_ref_abs_idx, Some(0));
    }

    // ---- outstanding section bookkeeping ----

    #[test]
    fn outstanding_section_tracks_ric_and_min_ref_across_multiple_refs() {
        let encoder = new_table(4096);
        let a = encoder.insert(qen("x-a"), fv("1")).unwrap();
        let b = encoder.insert(qen("x-b"), fv("2")).unwrap();
        let c = encoder.insert(qen("x-c"), fv("3")).unwrap();
        assert_eq!((a, b, c), (0, 1, 2));
        encoder.on_insert_count_increment(3).unwrap();

        let mut headers = Headers::new();
        headers.insert("x-b", "2");
        headers.insert("x-c", "3");
        let _ = encode(&encoder, PseudoHeaders::default(), &headers, 7);

        let os = outstanding(&encoder, 7);
        assert_eq!(os.len(), 1);
        assert_eq!(os[0].required_insert_count, 3);
        assert_eq!(os[0].min_ref_abs_idx, Some(1));
    }

    #[test]
    fn outstanding_section_not_registered_when_section_is_static_only() {
        let encoder = new_table(4096);
        encoder.insert(qen("x-a"), fv("1")).unwrap();
        encoder.on_insert_count_increment(1).unwrap();

        let _ = encode(
            &encoder,
            PseudoHeaders {
                method: Some(Method::Get),
                ..Default::default()
            },
            &Headers::new(),
            9,
        );
        assert!(outstanding(&encoder, 9).is_empty());
    }

    // ---- blocked streams budget ----

    #[test]
    fn blocked_budget_zero_falls_back_to_literal_when_unacknowledged() {
        let encoder = new_table_with_blocked_streams(4096, 0);
        encoder.insert(qen("x-custom"), fv("hello")).unwrap();
        // KRC stays at 0 — a reference would block.

        let mut headers = Headers::new();
        headers.insert("x-custom", "hello");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        assert!(
            matches!(
                lines.as_slice(),
                [FieldLineInstruction::LiteralLiteralName { .. }]
            ),
            "expected fallback to literal-with-literal-name, got {lines:?}",
        );
        assert!(outstanding(&encoder, 1).is_empty());
    }

    #[test]
    fn blocked_budget_already_blocking_stream_allows_additional_refs() {
        let encoder = new_table_with_blocked_streams(4096, 1);
        encoder.insert(qen("x-a"), fv("1")).unwrap();
        encoder.insert(qen("x-b"), fv("2")).unwrap();
        // KRC=0; both refs would block.
        encoder.register_outstanding_section(
            1,
            SectionRefs {
                required_insert_count: 1,
                min_ref_abs_idx: Some(0),
            },
        );

        let mut headers = Headers::new();
        headers.insert("x-b", "2");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let (prefix, _) = parse_section(&bytes);
        assert_ne!(prefix.encoded_required_insert_count, 0);
        assert_eq!(outstanding(&encoder, 1).len(), 2);
    }

    #[test]
    fn blocked_budget_new_stream_refused_when_full() {
        let encoder = new_table_with_blocked_streams(4096, 1);
        encoder.insert(qen("x-a"), fv("1")).unwrap();
        encoder.register_outstanding_section(
            1,
            SectionRefs {
                required_insert_count: 1,
                min_ref_abs_idx: Some(0),
            },
        );

        let mut headers = Headers::new();
        headers.insert("x-a", "1");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 2);

        let (prefix, _) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        assert!(outstanding(&encoder, 2).is_empty());
    }

    #[test]
    fn non_blocking_ref_allowed_when_budget_is_zero() {
        let encoder = new_table_with_blocked_streams(4096, 0);
        encoder.insert(qen("x-custom"), fv("hello")).unwrap();
        encoder.on_insert_count_increment(1).unwrap();

        let mut headers = Headers::new();
        headers.insert("x-custom", "hello");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(
            prefix,
            FieldSectionPrefix {
                encoded_required_insert_count: 2,
                base_is_negative: false,
                delta_base: 0,
            }
        );
        assert_eq!(
            lines,
            vec![FieldLineInstruction::IndexedDynamic { relative_index: 0 }]
        );
    }

    #[test]
    fn committed_to_blocking_covers_multiple_refs_in_same_section() {
        let encoder = new_table_with_blocked_streams(4096, 1);
        encoder.insert(qen("x-a"), fv("1")).unwrap();
        encoder.insert(qen("x-b"), fv("2")).unwrap();
        // KRC stays 0.

        let mut headers = Headers::new();
        headers.insert("x-a", "1");
        headers.insert("x-b", "2");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix.encoded_required_insert_count, 3);
        assert!(
            matches!(
                lines.as_slice(),
                [
                    FieldLineInstruction::IndexedDynamic { .. },
                    FieldLineInstruction::IndexedDynamic { .. },
                ],
            ),
            "expected two indexed dynamic refs, got {lines:?}",
        );
    }

    // ---- dynamic name reference (literal with dynamic name ref, §4.5.4 T=0) ----

    #[test]
    fn dynamic_name_ref_header_with_different_value() {
        let encoder = new_table(4096);
        encoder.insert(qen("x-custom"), fv("hello")).unwrap();
        encoder.on_insert_count_increment(1).unwrap();

        let mut headers = Headers::new();
        headers.insert("x-custom", "world");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(
            prefix,
            FieldSectionPrefix {
                encoded_required_insert_count: 2,
                base_is_negative: false,
                delta_base: 0,
            }
        );
        let [
            FieldLineInstruction::LiteralDynamicNameRef {
                relative_index: 0,
                value,
                never_indexed: false,
            },
        ] = lines.as_slice()
        else {
            panic!("expected literal-with-dynamic-name-ref, got {lines:?}");
        };
        assert_eq!(value.as_bytes(), b"world");

        let os = outstanding(&encoder, 1);
        assert_eq!(os.len(), 1);
        assert_eq!(os[0].required_insert_count, 1);
        assert_eq!(os[0].min_ref_abs_idx, Some(0));
    }

    #[test]
    fn dynamic_name_ref_budget_zero_falls_back_to_literal() {
        let encoder = new_table_with_blocked_streams(4096, 0);
        encoder.insert(qen("x-custom"), fv("hello")).unwrap();
        // KRC=0 → any dynamic reference would block.

        let mut headers = Headers::new();
        headers.insert("x-custom", "world");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        assert!(
            matches!(
                lines.as_slice(),
                [FieldLineInstruction::LiteralLiteralName { .. }]
            ),
            "expected fallback to literal-with-literal-name, got {lines:?}",
        );
        assert!(outstanding(&encoder, 1).is_empty());
    }

    #[test]
    fn dynamic_name_ref_already_blocking_stream_allows_ref() {
        // Capacity fits exactly one entry, so insert-then-reference cannot run (would evict
        // the pinned source). Falls through to dynamic name-ref.
        let encoder = new_table_with_blocked_streams(45, 1); // 8 + 5 + 32
        encoder.insert(qen("x-custom"), fv("hello")).unwrap();
        encoder.register_outstanding_section(
            1,
            SectionRefs {
                required_insert_count: 1,
                min_ref_abs_idx: Some(0),
            },
        );

        let mut headers = Headers::new();
        headers.insert("x-custom", "world");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let (prefix, lines) = parse_section(&bytes);
        assert_ne!(prefix.encoded_required_insert_count, 0);
        assert!(
            matches!(
                lines.as_slice(),
                [FieldLineInstruction::LiteralDynamicNameRef { .. }]
            ),
            "expected literal-with-dynamic-name-ref, got {lines:?}",
        );
    }

    #[test]
    fn static_name_ref_preferred_over_dynamic_name_ref() {
        let encoder = new_table(4096);
        encoder
            .insert(KnownHeaderName::ContentType.into(), fv("application/json"))
            .unwrap();
        encoder.on_insert_count_increment(1).unwrap();

        let mut headers = Headers::new();
        headers.insert(KnownHeaderName::ContentType, "application/xml");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        let [
            FieldLineInstruction::LiteralStaticNameRef {
                value,
                never_indexed: false,
                ..
            },
        ] = lines.as_slice()
        else {
            panic!("expected literal-with-static-name-ref, got {lines:?}");
        };
        assert_eq!(value.as_bytes(), b"application/xml");
        assert!(outstanding(&encoder, 1).is_empty());
    }

    #[test]
    fn dynamic_name_ref_protocol_pseudo() {
        let encoder = new_table(4096);
        encoder
            .insert(
                QpackEntryName::Pseudo(PseudoHeaderName::Protocol),
                fv("webtransport"),
            )
            .unwrap();
        encoder.on_insert_count_increment(1).unwrap();

        let bytes = encode(
            &encoder,
            PseudoHeaders {
                protocol: Some(Cow::Borrowed("connect-udp")),
                ..Default::default()
            },
            &Headers::new(),
            1,
        );

        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(
            prefix,
            FieldSectionPrefix {
                encoded_required_insert_count: 2,
                base_is_negative: false,
                delta_base: 0,
            }
        );
        let [
            FieldLineInstruction::LiteralDynamicNameRef {
                relative_index: 0,
                value,
                never_indexed: false,
            },
        ] = lines.as_slice()
        else {
            panic!("expected literal-with-dynamic-name-ref, got {lines:?}");
        };
        assert_eq!(value.as_bytes(), b"connect-udp");
    }

    #[test]
    fn committed_to_blocking_covers_full_match_then_name_ref() {
        // Capacity for exactly two entries so insert-then-reference for the second ref would
        // need to evict the pinned abs 0 — blocked, falls through to dynamic name-ref.
        //
        // `Headers` iterates its unknown-name HashMap in non-deterministic order, so we
        // assert on the set of field-line kinds observed rather than their position.
        let encoder = new_table_with_blocked_streams(72, 1); // 2 × (3 + 1 + 32)
        encoder.insert(qen("x-a"), fv("1")).unwrap();
        encoder.insert(qen("x-b"), fv("1")).unwrap();

        let mut headers = Headers::new();
        headers.insert("x-a", "1");
        headers.insert("x-b", "2");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let (prefix, lines) = parse_section(&bytes);
        // max_capacity=72 → max_entries=2; RIC=2 → encoded_ric = (2 % 4) + 1 = 3.
        assert_eq!(prefix.encoded_required_insert_count, 3);

        // Exactly one indexed-dynamic-pre-base (x-a:1 full match) and one
        // literal-with-dynamic-name-ref (x-b:2 falls through after insert fails). Order is
        // not deterministic because `Headers` iterates its unknown-name HashMap arbitrarily.
        let has_indexed_dynamic = lines
            .iter()
            .any(|l| matches!(l, FieldLineInstruction::IndexedDynamic { .. }));
        let has_dynamic_name_ref = lines
            .iter()
            .any(|l| matches!(l, FieldLineInstruction::LiteralDynamicNameRef { .. }));
        assert!(
            has_indexed_dynamic,
            "expected an indexed dynamic in {lines:?}"
        );
        assert!(
            has_dynamic_name_ref,
            "expected a dynamic name-ref in {lines:?}",
        );
        assert_eq!(lines.len(), 2);

        let os = outstanding(&encoder, 1);
        assert_eq!(os.len(), 1);
        assert_eq!(os[0].required_insert_count, 2);
    }

    #[test]
    fn roundtrip_dynamic_name_ref_through_reader() {
        let encoder = new_table(4096);
        // The encoder's init SetCapacity will be drained into the decoder below via
        // `roundtrip`, so the decoder is constructed uninitialized.
        let decoder = DecoderDynamicTable::new(4096, 0);

        encoder.insert(qen("x-custom"), fv("hello")).unwrap();
        encoder.on_insert_count_increment(1).unwrap();

        let mut headers = Headers::new();
        headers.insert("x-custom", "world");
        let decoded = roundtrip(&encoder, &decoder, PseudoHeaders::default(), &headers, 1);
        let (_, decoded_headers) = decoded.into_parts();
        assert_eq!(decoded_headers.get_str("x-custom"), Some("world"));
    }

    // ---- multi-value and duplicate-value headers ----

    #[test]
    fn multi_value_header_emits_one_field_line_per_value() {
        let encoder = new_table(4096);
        let mut headers = Headers::new();
        headers.append("x-multi", "first");
        headers.append("x-multi", "second");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let decoder = DecoderDynamicTable::new(4096, 0);
        let decoded = block_on(decoder.decode(&bytes, 1)).unwrap();
        let (_, decoded_headers) = decoded.into_parts();
        let values: Vec<&str> = decoded_headers
            .get_values("x-multi")
            .map(|vs| vs.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(values, vec!["first", "second"]);
    }

    #[test]
    fn section_prefix_encodes_nonzero_ric_with_delta_base_zero() {
        let encoder = new_table(4096);
        for i in 0..5u32 {
            encoder
                .insert(qen("x-custom"), fvo(i.to_string().into_bytes()))
                .unwrap();
        }
        encoder.on_insert_count_increment(5).unwrap();

        let mut headers = Headers::new();
        headers.insert("x-custom", "4");
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

        let (prefix, _) = parse_section(&bytes);
        // max_entries = 4096 / 32 = 128, so encoded_ric = (5 % 256) + 1 = 6.
        assert_eq!(
            prefix,
            FieldSectionPrefix {
                encoded_required_insert_count: 6,
                base_is_negative: false,
                delta_base: 0,
            }
        );
    }

    #[test]
    fn roundtrip_dynamic_full_match_through_reader() {
        let encoder = new_table(4096);
        // The encoder's init SetCapacity will be drained into the decoder below via
        // `roundtrip`, so the decoder is constructed uninitialized.
        let decoder = DecoderDynamicTable::new(4096, 0);

        encoder.insert(qen("x-custom"), fv("hello")).unwrap();
        encoder.on_insert_count_increment(1).unwrap();

        let mut headers = Headers::new();
        headers.insert("x-custom", "hello");
        let decoded = roundtrip(&encoder, &decoder, PseudoHeaders::default(), &headers, 1);
        let (_, decoded_headers) = decoded.into_parts();
        assert_eq!(decoded_headers.get_str("x-custom"), Some("hello"));
    }
}
