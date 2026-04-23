//! Tests for the dup-draining refresh pass (`Planner::refresh_hot_tail`).
//!
//! The pass fires inside the warming-insert step of `plan_emission`, gated on
//! `headroom < primed_bytes`. When open, it checks whether the oldest table entry is
//! still observer-hot and, if so, emits a Duplicate instruction before the warming
//! insert proceeds — so the hot primed entry survives the eviction that the insert
//! would otherwise trigger.

use super::*;

/// Build a context whose observer has seen each `(name, value)` pair once. The
/// simplified observer treats every observed pair as hot for the lifetime of
/// the listener, so a single observation is sufficient to make `prime()` emit
/// it as a candidate.
fn context_with_observations(
    max_capacity: usize,
    observations: &[(EntryName<'static>, FieldLineValue<'static>)],
) -> HttpContext {
    let context = HttpContext::default()
        .with_config(crate::HttpConfig::default().with_h3_max_table_capacity(max_capacity));
    let mut accum = ConnectionAccumulator::default();
    for (name, value) in observations {
        accum.observe(name, value);
    }
    context.observer.fold_connection(&accum);
    context
}

fn init_table(context: &HttpContext, max_capacity: u64) -> EncoderDynamicTable {
    let table = EncoderDynamicTable::new(context);
    table.initialize_from_peer_settings(
        H3Settings::default()
            .with_qpack_max_table_capacity(max_capacity)
            .with_qpack_blocked_streams(10),
    );
    table
}

fn count_duplicates(ops: &[EncoderInstruction]) -> usize {
    ops.iter()
        .filter(|op| matches!(op, EncoderInstruction::Duplicate { .. }))
        .count()
}

#[test]
fn gate_closed_when_no_priming() {
    // Fresh observer, no observations — prime() returns empty, primed_bytes stays 0.
    let context = HttpContext::default()
        .with_config(crate::HttpConfig::default().with_h3_max_table_capacity(160));
    let table = init_table(&context, 160);
    assert_eq!(
        table.entry_count(),
        0,
        "no observations → no priming inserts"
    );

    // Encode a section that would trigger a warming insert. With primed_bytes = 0, the
    // gate never opens regardless of headroom; no Duplicate should be emitted.
    let _ = drain_instructions(&table);
    let lines = vec![(qen("x-w1"), fv("v")), (qen("x-w1"), fv("v"))];
    let mut buf = Vec::new();
    table.encode_field_lines(&lines, &mut buf, 1);

    let ops = drain_instructions(&table);
    assert_eq!(
        count_duplicates(&ops),
        0,
        "no Duplicate expected without priming, got {ops:?}",
    );
}

#[test]
fn gate_closed_when_table_has_headroom() {
    // Prime one hot pair into a very large table. Headroom stays far above
    // `primed_bytes` after warming inserts, so the gate never opens.
    let server = (
        EntryName::Known(KnownHeaderName::Server),
        FieldLineValue::Static(b"trillium"),
    );
    let context = context_with_observations(4096, &[server]);
    let table = init_table(&context, 4096);
    assert!(
        table.entry_count() >= 1,
        "expected the server primed entry to land, got {}",
        table.entry_count()
    );

    let _ = drain_instructions(&table);
    let lines = vec![(qen("x-w1"), fv("v")), (qen("x-w1"), fv("v"))];
    let mut buf = Vec::new();
    table.encode_field_lines(&lines, &mut buf, 1);

    let ops = drain_instructions(&table);
    assert_eq!(
        count_duplicates(&ops),
        0,
        "no Duplicate expected with plenty of headroom, got {ops:?}",
    );
}

// `gate_open_cold_tail_no_dup` removed: the simplified observer has no decay
// path. Once a pair is observed it stays in the priming set for the listener's
// lifetime, so `is_hot` cannot transition from true to false. The dup-drain
// refresh path is exercised by `gate_open_hot_tail_emits_duplicate` and
// `sustained_pressure_emits_dup_and_remains_consistent`; the not-hot branch is
// exercised by `gate_closed_when_no_priming` (observer was never fed).

#[test]
fn gate_open_hot_tail_emits_duplicate() {
    // Prime (server, trillium) and (user-agent, custom); capacity 160 is tight enough
    // that a warming insert opens the gate on the first attempt. The oldest entry is
    // still observer-hot, so a Duplicate is emitted before the warming insert.
    let server = (
        EntryName::Known(KnownHeaderName::Server),
        FieldLineValue::Static(b"trillium"),
    );
    let ua = (
        EntryName::Known(KnownHeaderName::UserAgent),
        FieldLineValue::Static(b"custom"),
    );
    let context = context_with_observations(160, &[server, ua]);
    let table = init_table(&context, 160);
    assert!(
        table.entry_count() >= 2,
        "expected both pairs primed, got entry_count = {}",
        table.entry_count(),
    );
    let primed_count = table.insert_count();
    let _ = drain_instructions(&table);

    let lines = vec![(qen("x-w1"), fv("v")), (qen("x-w1"), fv("v"))];
    let mut buf = Vec::new();
    table.encode_field_lines(&lines, &mut buf, 1);

    let ops = drain_instructions(&table);
    assert!(
        count_duplicates(&ops) >= 1,
        "expected a Duplicate from dup-drain, got {ops:?}",
    );
    assert!(
        table.insert_count() >= primed_count + 2,
        "expected dup + warming insert; primed_count={primed_count}, now={}",
        table.insert_count(),
    );
}

#[test]
fn sustained_pressure_emits_dup_and_remains_consistent() {
    // Multiple successive warming-insert bursts — the first (at least) should refresh
    // a hot primed entry via Duplicate. Further rounds may succeed or silently fail
    // depending on whether the table's remaining tail still has an older entry that
    // eviction can touch (the structural "nothing below the pin" case). In either
    // case the warming insert itself always proceeds and the table remains consistent.
    let server = (
        EntryName::Known(KnownHeaderName::Server),
        FieldLineValue::Static(b"trillium"),
    );
    let ua = (
        EntryName::Known(KnownHeaderName::UserAgent),
        FieldLineValue::Static(b"custom"),
    );
    let context = context_with_observations(160, &[server, ua]);
    let table = init_table(&context, 160);
    let _ = drain_instructions(&table);

    let primed = table.insert_count();
    let mut total_dups = 0;
    for (stream_id, new_name) in [(1u64, "x-w1"), (2, "x-w2"), (3, "x-w3")] {
        let lines = vec![(qen(new_name), fv("v")), (qen(new_name), fv("v"))];
        let mut buf = Vec::new();
        table.encode_field_lines(&lines, &mut buf, stream_id);
        let ops = drain_instructions(&table);
        total_dups += count_duplicates(&ops);
    }
    assert!(
        total_dups >= 1,
        "expected at least one Duplicate under sustained pressure, got {total_dups}",
    );
    // Three warming inserts landed; the first round also added a Duplicate, giving at
    // least primed + 3 + 1 entries. Upper bound allows for the dup + warming-insert
    // successes across later rounds.
    let grew_by = table.insert_count() - primed;
    assert!(
        grew_by >= 4,
        "expected at least 4 inserts (3 warming + ≥1 dup), got {grew_by}",
    );
}
