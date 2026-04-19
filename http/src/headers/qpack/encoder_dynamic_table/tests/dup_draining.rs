//! Phase-4 step-5 tests: the post-encode `dup_draining` pass.
//!
//! Building a state where the pass has something to do takes some care. The constraints
//! are delicate:
//!
//! - The candidate entry must be in the draining region (oldest end of a near-full table).
//! - Its `nameval_hash` must be in the predictor ring — so something must have encoded
//!   that exact `(name, value)` through the planner (not a direct `table.insert`).
//! - `safe_to_dup` must succeed — no outstanding section may pin the candidate at or below
//!   its `abs_idx`, and there must be room for the new copy (free space or older-evictable).
//! - The candidate must still be the latest copy of its `(name, value)` pair (the pass
//!   skips entries whose `by_value` entry already points elsewhere).
//!
//! The test helpers below set up a canonical shape: a draining single-entry target at abs
//! 0 (its predictor slot primed via a real encode pair), plus filler pushing the table
//! near-full. The ack-then-fill-directly pattern avoids polluting the ring with the filler
//! pairs and clears the section-level pin on the target so `safe_to_dup` passes.

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

fn encode_headers(
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

/// Plant a dynamic-table entry that the predictor has also observed. Encodes the same
/// `(name, value)` twice: the first sighting is a literal that primes the predictor, the
/// second hits `insert-then-reference` and seats the entry at the table's head. Acks the
/// resulting section so no section-level pin remains on the new entry. Drains the
/// encoder stream so the test body sees only the output of the call it's actually
/// exercising.
fn plant_primed_entry(
    table: &EncoderDynamicTable,
    name: &'static str,
    value: &'static str,
    stream_id: u64,
) {
    let mut headers = Headers::new();
    headers.insert(name, value);
    let _ = encode_headers(table, PseudoHeaders::default(), &headers, stream_id);
    let _ = encode_headers(table, PseudoHeaders::default(), &headers, stream_id);
    table.on_section_ack(stream_id).unwrap();
    let _ = drain_instructions(table);
}

/// Directly insert `count` entries `("z", "0"), ("z", "1"), …` via the low-level API so
/// the predictor ring stays clean. Advances KRC so the resulting entries are non-blocking
/// (not strictly required for [`TableState::dup_draining_pass`], but keeps downstream
/// lookups simple). `count` must be ≤ 10 so single-char suffixes `"0"`–`"9"` suffice for
/// uniqueness.
fn fill_directly(table: &EncoderDynamicTable, count: usize) {
    assert!(count <= 10);
    for i in 0..count {
        let v: &'static str = match i {
            0 => "0",
            1 => "1",
            2 => "2",
            3 => "3",
            4 => "4",
            5 => "5",
            6 => "6",
            7 => "7",
            8 => "8",
            9 => "9",
            _ => unreachable!(),
        };
        table.insert(qen("z"), fv(v)).unwrap();
    }
    table
        .on_insert_count_increment(count as u64)
        .unwrap();
    let _ = drain_instructions(table);
}

/// Run a dummy encode through the planner so `encode_field_lines` runs, triggering the
/// post-encode [`TableState::dup_draining_pass`]. Uses a fresh `(name, value)` that
/// doesn't match anything in the table so the section itself is a literal — the only
/// non-trivial encoder-stream bytes it contributes will be from the pass.
fn trigger_post_encode_pass(table: &EncoderDynamicTable, stream_id: u64) {
    let mut headers = Headers::new();
    headers.insert("trigger", "tv");
    let _ = encode_headers(table, PseudoHeaders::default(), &headers, stream_id);
}

#[test]
fn dup_draining_fires_on_hot_draining_entry() {
    // capacity 400, 10 × 34-byte entries → 340 B used, 60 B free. Threshold = 100.
    //   abs 0: cumulative = 60+34 = 94 < 100 → draining.
    //   abs 1: cumulative = 94+34 = 128 ≥ 100 → frontier.
    // So abs 0 is the sole draining candidate. `safe_to_dup(abs 0, None)` fits the new
    // copy in the 60 B of free space (34 ≤ 60) without any eviction.
    let table = new_table_with_mnemonic(400);
    let _ = drain_instructions(&table);

    plant_primed_entry(&table, "x", "target", 1); // seats abs 0 = ("x", "target")
    fill_directly(&table, 9); // abs 1..9 = ("z", "0".."8"), insert_count=10

    let before = table.insert_count();

    trigger_post_encode_pass(&table, 2);

    let instrs = drain_instructions(&table);
    assert!(
        instrs
            .iter()
            .any(|i| matches!(i, EncoderInstruction::Duplicate { .. })),
        "expected a Duplicate from dup_draining, got {instrs:?}",
    );
    assert_eq!(
        table.insert_count(),
        before + 1,
        "exactly one Duplicate expected (abs 0 has no newer copy after first dup)",
    );
}

#[test]
fn dup_draining_skipped_without_mnemonic() {
    // Same draining shape but mnemonic is off — the predictor ring is never consulted,
    // and the pass returns immediately. The trigger encode itself may still insert
    // something via the regular planner (mnemonic off means eager indexing), but no
    // *Duplicate* instruction may appear.
    let table = new_table(400); // mnemonic off
    let _ = drain_instructions(&table);

    table.insert(qen("x"), fv("target")).unwrap();
    fill_directly(&table, 9);

    trigger_post_encode_pass(&table, 1);

    let instrs = drain_instructions(&table);
    assert!(
        !instrs
            .iter()
            .any(|i| matches!(i, EncoderInstruction::Duplicate { .. })),
        "no Duplicate expected when mnemonic is off, got {instrs:?}",
    );
}

#[test]
fn dup_draining_skips_entry_with_newer_copy() {
    // Setup leaves abs 0 primed as ("x", "target"). Before the trigger encode, we encode
    // ("x", "target") one more time — it matches abs 0, but abs 0 is draining, so step
    // 2's main-path refresh fires and produces a Duplicate of its own. Now there are two
    // ("x", "target") entries: abs 0 (draining, stale per `by_value`) and abs 10 (fresh,
    // at the table head). When the trigger encode runs post-encode dup_draining, abs 0
    // is skipped because `by_value[("x","target")] == Some(abs 10)`.
    let table = new_table_with_mnemonic(400);
    let _ = drain_instructions(&table);

    plant_primed_entry(&table, "x", "target", 1);
    fill_directly(&table, 9);

    // Force a newer copy of abs 0 via step 2's main-path refresh.
    let mut h = Headers::new();
    h.insert("x", "target");
    let _ = encode_headers(&table, PseudoHeaders::default(), &h, 2);
    table.on_section_ack(2).unwrap();
    let _ = drain_instructions(&table);

    let before = table.insert_count();
    trigger_post_encode_pass(&table, 3);

    let instrs = drain_instructions(&table);
    assert!(
        !instrs
            .iter()
            .any(|i| matches!(i, EncoderInstruction::Duplicate { .. })),
        "no Duplicate expected — abs 0 has a newer copy, no other draining entry has a \
         predictor hit. Got {instrs:?}",
    );
    assert_eq!(
        table.insert_count(),
        before,
        "no inserts expected from the trigger encode's post-pass",
    );
}

#[test]
fn dup_draining_picks_biggest_first() {
    // Two primed entries at the oldest end, both draining and predictor-hit:
    //   abs 0: ("yyyy", "bigvalue") → 4 + 8 + 32 = 44 B   (bigger, picked first)
    //   abs 1: ("x", "target")      → 1 + 6 + 32 = 39 B
    // Plus 12 × 36 B fillers (name `"zz"`, two-char values) = 432 B.
    // total = 44 + 39 + 432 = 515 B at cap = 560 → free = 45, threshold = 140.
    //   abs 0: cumulative = 45 + 44 = 89  < 140 → draining.
    //   abs 1: cumulative = 89 + 39 = 128 < 140 → draining.
    //   abs 2: cumulative = 128 + 36 = 164 ≥ 140 → frontier.
    // Both primed entries are draining. `safe_to_dup` fits both without eviction
    // (current + 44 = 559 ≤ 560 for abs 0; abs 1 is even smaller).
    let table = new_table_with_mnemonic(560);
    let _ = drain_instructions(&table);

    plant_primed_entry(&table, "yyyy", "bigvalue", 1); // abs 0, 44 B
    plant_primed_entry(&table, "x", "target", 2); // abs 1, 39 B
    for v in [
        "aa", "bb", "cc", "dd", "ee", "ff", "gg", "hh", "ii", "jj", "kk", "ll",
    ] {
        table.insert(qen("zz"), fv(v)).unwrap();
    }
    table.on_insert_count_increment(12).unwrap();
    let _ = drain_instructions(&table);
    assert_eq!(table.insert_count(), 14);

    trigger_post_encode_pass(&table, 3);

    let instrs = drain_instructions(&table);
    let dups: Vec<_> = instrs
        .iter()
        .filter_map(|i| match i {
            EncoderInstruction::Duplicate { relative_index } => Some(*relative_index),
            _ => None,
        })
        .collect();

    // Pass makes one Duplicate per loop iteration. Expected two:
    //   1. abs 0 picked first (bigger). insert_count before dup = 14 → relative = 13.
    //   2. abs 1 picked next (abs 0 now has a newer copy so it's skipped). insert_count
    //      before second dup = 15 → relative = 15 - 1 - 1 = 13 as well (different entry,
    //      same relative offset because the new entry from step 1 shifted everything).
    // Match on the sequence to catch both order and count.
    assert_eq!(
        dups,
        vec![13, 13],
        "expected two Duplicates targeting abs 0 then abs 1, all instrs = {instrs:?}",
    );
}
