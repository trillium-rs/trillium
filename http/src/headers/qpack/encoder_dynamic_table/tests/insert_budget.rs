//! Phase-7 per-encode insert budget: once a section has inserted `capacity / 2` bytes
//! of new dynamic-table entries, further lines in that section bypass the dynamic table
//! entirely (no inserts, no references). Mirrors ls-qpack's `enc_use_dynamic_table`.

use super::*;
use crate::headers::qpack::instruction::encoder::EncoderInstruction;

/// A section whose cumulative insert size stays below `capacity / 2` should be free to
/// insert every header. Mnemonic off so each indexable header takes the eager
/// insert-then-reference path on first sighting.
#[test]
fn under_budget_section_inserts_every_header() {
    // Capacity 4096 → budget 2048 bytes. Three small entries (~40 bytes each via the
    // `+ 32` overhead) come nowhere near.
    let encoder = new_table_configured(4096, 100, false, 1.0);
    let _ = drain_instructions(&encoder);

    let mut buf = Vec::new();
    let lines = vec![
        (qen("x-a"), fv("1")),
        (qen("x-b"), fv("2")),
        (qen("x-c"), fv("3")),
    ];
    encoder.encode_field_lines(&lines, &mut buf, 1);

    let inserts = drain_instructions(&encoder)
        .into_iter()
        .filter(|i| matches!(i, EncoderInstruction::InsertWithLiteralName { .. }))
        .count();
    assert_eq!(inserts, 3, "all three headers should insert under budget");
}

/// A section large enough to cross `capacity / 2` should stop inserting partway through.
/// Verifies the budget actually fires.
#[test]
fn over_budget_section_stops_inserting_after_threshold() {
    // Capacity 256 → budget 128 bytes. Each entry is `name.len() + value.len() + 32`,
    // so 5 entries of ~50 bytes are ~250 bytes total — well over the 128-byte budget.
    let encoder = new_table_configured(256, 100, false, 1.0);
    let _ = drain_instructions(&encoder);

    let names: Vec<String> = (0..5).map(|i| format!("x-name-{i:02}")).collect();
    let values: Vec<String> = (0..5).map(|i| format!("value-no-{i:02}")).collect();
    let lines: Vec<_> = names
        .iter()
        .zip(values.iter())
        .map(|(n, v)| (qen(n), fvo(v.as_bytes().to_vec())))
        .collect();
    let mut buf = Vec::new();
    encoder.encode_field_lines(&lines, &mut buf, 1);

    let inserts: usize = drain_instructions(&encoder)
        .into_iter()
        .filter(|i| matches!(i, EncoderInstruction::InsertWithLiteralName { .. }))
        .count();
    assert!(
        inserts < 5,
        "budget should suppress at least one insert, got {inserts}",
    );
    assert!(
        inserts >= 1,
        "budget should still allow some inserts before tripping, got {inserts}",
    );
}

/// Sanity: the budget resets between sections. A section that hits the cap shouldn't
/// poison the next section's ability to insert.
#[test]
fn budget_resets_per_section() {
    let encoder = new_table_configured(256, 100, false, 1.0);
    let _ = drain_instructions(&encoder);

    let names: Vec<String> = (0..5).map(|i| format!("x-fill-{i:02}")).collect();
    let values: Vec<String> = (0..5).map(|i| format!("blob-zzzz{i:02}")).collect();
    let fill_lines: Vec<_> = names
        .iter()
        .zip(values.iter())
        .map(|(n, v)| (qen(n), fvo(v.as_bytes().to_vec())))
        .collect();
    let mut buf = Vec::new();
    encoder.encode_field_lines(&fill_lines, &mut buf, 1);
    let _ = drain_instructions(&encoder);

    // Second section: a single fresh insert should be allowed (counter reset).
    let mut buf = Vec::new();
    encoder.encode_field_lines(&[(qen("x-fresh"), fv("v"))], &mut buf, 3);
    let inserts: usize = drain_instructions(&encoder)
        .into_iter()
        .filter(|i| matches!(i, EncoderInstruction::InsertWithLiteralName { .. }))
        .count();
    assert_eq!(inserts, 1, "counter should reset between sections");
}
