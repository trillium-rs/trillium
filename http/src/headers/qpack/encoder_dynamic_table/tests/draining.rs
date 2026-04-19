//! Phase-4 draining tests.
//!
//! - Step 1 (below, "table-state helpers"): `TableState::draining_frontier_abs_idx` and
//!   `TableState::safe_to_dup` — raw computations that the policy depends on.
//! - Step 2 (below, "main-path DUP-on-draining"): planner behaviour when a full match lands
//!   on a draining entry. Each test constructs a near-full table, makes abs 0 draining, and
//!   then drives an encode that matches that entry.

use super::*;
use crate::{
    Headers,
    headers::qpack::{
        FieldSection, PseudoHeaders,
        instruction::{
            encoder::EncoderInstruction,
            field_section::{FieldLineInstruction, FieldSectionPrefix},
        },
    },
};

/// Entry size for a `qen("x") / fv("y")` pair: 1-byte name + 1-byte value + 32 RFC-mandated
/// overhead.
const SMALL_ENTRY: usize = 34;

#[test]
fn draining_frontier_on_empty_table_is_zero() {
    let table = new_table(4096);
    assert_eq!(table.state.lock().unwrap().draining_frontier_abs_idx(), 0);
}

#[test]
fn draining_frontier_with_plenty_of_free_space_is_zero() {
    // Capacity 4096, two small entries — free space alone (4096 - 68 = 4028) dwarfs the
    // `capacity/4 = 1024` threshold, so nothing drains.
    let table = new_table(4096);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    assert_eq!(table.state.lock().unwrap().draining_frontier_abs_idx(), 0);
}

#[test]
fn draining_frontier_marks_prefix_on_full_table() {
    // Capacity 400, 11 entries of 34 bytes = 374 used, 26 free. Threshold = 100.
    //
    // For entry at position i (oldest = 0), cumulative = free_space + (i+1) * size. Draining
    // when cumulative < 100:
    //   i=0: 26 + 34 = 60  < 100 → draining
    //   i=1: 26 + 68 = 94  < 100 → draining
    //   i=2: 26 + 102 = 128 ≥ 100 → frontier
    //
    // Expected frontier abs_idx = 2 (abs 0 and 1 draining; 2..10 not).
    let table = new_table(400);
    // 11 entries, each 34 bytes (single-char name + single-char value + 32 overhead). Share
    // name `"x"`; distinct value chars make each `(name, value)` unique.
    for value in ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "a"] {
        table.insert(qen("x"), fv(value)).unwrap();
    }
    let state = table.state.lock().unwrap();
    assert_eq!(state.entries.len(), 11);
    assert_eq!(state.current_size, 11 * SMALL_ENTRY);
    assert_eq!(state.draining_frontier_abs_idx(), 2);
}

#[test]
fn draining_frontier_exact_threshold_is_non_draining() {
    // Capacity chosen so the second-oldest entry sits exactly at the threshold. `dist <
    // threshold` is a strict inequality: equal-to-threshold is NOT draining.
    //
    // Target: (i+1) * 34 + free_space == capacity/4 at i=1 (second-oldest).
    // capacity/4 = 68 + free_space; capacity = 272 + 4*free_space.
    //
    // Pick free_space = 0 → capacity = 272, which fits 8 entries (272 bytes). Check:
    //   threshold = 68. i=0: cumulative=34 < 68 draining. i=1: 68 >= 68 → frontier at 1.
    let table = new_table(272);
    for name in ["a", "b", "c", "d", "e", "f", "g", "h"] {
        table.insert(qen(name), fv("v")).unwrap();
    }
    let state = table.state.lock().unwrap();
    assert_eq!(state.current_size, 8 * SMALL_ENTRY);
    assert_eq!(state.draining_frontier_abs_idx(), 1);
}

#[test]
fn safe_to_dup_when_new_copy_fits_without_eviction() {
    // Plenty of headroom — duplicating anything succeeds.
    let table = new_table(4096);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    let state = table.state.lock().unwrap();
    assert!(state.safe_to_dup(0, None));
    assert!(state.safe_to_dup(1, None));
}

#[test]
fn safe_to_dup_when_older_entry_can_be_evicted() {
    // Capacity 100 fits 2 entries of 34 bytes (68 used, 32 free). A duplicate costs 34 more
    // → 102 > 100, need to evict. Duplicating abs 1 (newer) is safe: abs 0 is older and can
    // be dropped; simulated_used goes 68 → 34, then 34 + 34 = 68 ≤ 100.
    let table = new_table(100);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    let state = table.state.lock().unwrap();
    assert!(state.safe_to_dup(1, None));
}

#[test]
fn safe_to_dup_rejects_when_source_would_evict_itself() {
    // Same setup. Duplicating abs 0 (oldest) would require evicting abs 0 itself to make
    // room — unsafe.
    let table = new_table(100);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    let state = table.state.lock().unwrap();
    assert!(!state.safe_to_dup(0, None));
}

#[test]
fn safe_to_dup_rejects_when_outstanding_floor_blocks_eviction() {
    // Same table. Pin abs 0 via an outstanding section — eviction of the older entry is
    // now forbidden, so duplicating abs 1 also becomes unsafe (no other older entry exists
    // to evict).
    let table = new_table(100);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    table.register_outstanding_section(
        4,
        SectionRefs {
            required_insert_count: 1,
            min_ref_abs_idx: Some(0),
        },
    );
    let state = table.state.lock().unwrap();
    assert!(!state.safe_to_dup(1, None));
}

#[test]
fn safe_to_dup_respects_extra_floor() {
    // `extra_floor` threads the planner's in-progress `min_ref_abs_idx` through. With no
    // outstanding section but `extra_floor = Some(0)`, duplicating abs 1 can't evict abs 0
    // — matches the protection the real `duplicate` call would apply via `combine_floor`.
    let table = new_table(100);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    let state = table.state.lock().unwrap();
    assert!(!state.safe_to_dup(1, Some(0)));
}

#[test]
fn safe_to_dup_for_missing_abs_idx_is_false() {
    // An abs_idx that was evicted (or never existed) returns false instead of panicking.
    let table = new_table(100);
    table.insert(qen("a"), fv("1")).unwrap();
    let state = table.state.lock().unwrap();
    assert!(!state.safe_to_dup(42, None));
}

// -----------------------------------------------------------------------------
// Step 2 — main-path DUP-on-draining.
// -----------------------------------------------------------------------------

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

/// Fill the table to 11 × 34-byte entries at capacity 400. With free space 26 bytes and
/// threshold `capacity/4 = 100`, abs 0 and abs 1 are draining; abs 2..10 are not.
/// Duplicating abs 1 is safe — evicting abs 0 frees enough room.
fn fill_with_draining_prefix(table: &EncoderDynamicTable) {
    for name in [
        "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k",
    ] {
        table.insert(qen(name), fv("v")).unwrap();
    }
}

#[test]
fn non_draining_full_match_takes_fast_path_without_duplicate() {
    // Tiny table with a single entry — nothing is draining. Full match → plain reference,
    // no encoder-stream side effect.
    let table = new_table(4096);
    let _ = drain_instructions(&table);
    table.insert(qen("x"), fv("1")).unwrap();
    table.on_insert_count_increment(1).unwrap();
    let _ = drain_instructions(&table);

    let mut headers = Headers::new();
    headers.insert("x", "1");
    let bytes = encode(&table, PseudoHeaders::default(), &headers, 1);

    let (_prefix, lines) = parse_section(&bytes);
    assert!(
        matches!(lines.as_slice(), [FieldLineInstruction::IndexedDynamic { .. }]),
        "expected IndexedDynamic, got {lines:?}",
    );
    let instrs = drain_instructions(&table);
    assert!(instrs.is_empty(), "fast path should emit nothing, got {instrs:?}");
}

#[test]
fn draining_full_match_emits_duplicate_with_existing_reference() {
    // Full match against a draining entry (abs 1) that is safe to duplicate. Expect:
    // - section uses IndexedDynamic referencing the *existing* abs 1 (cheap ref);
    // - encoder stream carries a Duplicate whose relative index points at abs 1.
    let table = new_table(400);
    let _ = drain_instructions(&table);
    fill_with_draining_prefix(&table);
    table.on_insert_count_increment(11).unwrap();
    let _ = drain_instructions(&table); // drain the 11 Insert ops

    let mut headers = Headers::new();
    headers.insert("b", "v");
    let bytes = encode(&table, PseudoHeaders::default(), &headers, 1);

    let (_prefix, lines) = parse_section(&bytes);
    assert!(
        matches!(lines.as_slice(), [FieldLineInstruction::IndexedDynamic { .. }]),
        "expected IndexedDynamic, got {lines:?}",
    );

    let instrs = drain_instructions(&table);
    assert!(
        instrs
            .iter()
            .any(|i| matches!(i, EncoderInstruction::Duplicate { .. })),
        "expected Duplicate on encoder stream, got {instrs:?}",
    );
    // A new entry should be in the table now.
    assert_eq!(table.insert_count(), 12);
}

#[test]
fn draining_full_match_without_safe_to_dup_plain_reference() {
    // Pin abs 0 via an outstanding section so duplicating abs 1 would need to evict a
    // protected entry. `safe_to_dup` returns false, the step-2 refresh branch is skipped,
    // and the fast path takes over: plain reference, no Duplicate.
    let table = new_table(400);
    let _ = drain_instructions(&table);
    fill_with_draining_prefix(&table);
    table.on_insert_count_increment(11).unwrap();
    table.register_outstanding_section(
        999,
        SectionRefs {
            required_insert_count: 1,
            min_ref_abs_idx: Some(0),
        },
    );
    let _ = drain_instructions(&table);

    let mut headers = Headers::new();
    headers.insert("b", "v");
    let bytes = encode(&table, PseudoHeaders::default(), &headers, 1);

    let (_prefix, lines) = parse_section(&bytes);
    assert!(
        matches!(lines.as_slice(), [FieldLineInstruction::IndexedDynamic { .. }]),
        "expected IndexedDynamic, got {lines:?}",
    );
    let instrs = drain_instructions(&table);
    assert!(
        !instrs
            .iter()
            .any(|i| matches!(i, EncoderInstruction::Duplicate { .. })),
        "expected no Duplicate when safe_to_dup is false, got {instrs:?}",
    );
    // Insert count stays at 11 — no refresh happened.
    assert_eq!(table.insert_count(), 11);
}
