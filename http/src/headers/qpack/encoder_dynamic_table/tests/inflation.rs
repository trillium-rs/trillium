//! Phase-5 inflation guard: when the running `bytes_out / bytes_in` ratio would exceed
//! `inflation_ratio_max`, Insert-paired-with-literal programs (warming insert, name-only
//! insert) are suppressed and the line re-plans with indexing disabled.

use super::{drain_instructions, fv, new_table_configured, qen};
use crate::headers::qpack::encoder_dynamic_table::EncoderDynamicTable;

fn encode_one(
    encoder: &EncoderDynamicTable,
    name: &'static str,
    value: &'static str,
    stream_id: u64,
) {
    let lines = vec![(qen(name), fv(value))];
    let mut buf = Vec::new();
    encoder.encode_field_lines(&lines, &mut buf, stream_id);
}

/// Seed the running counters to a chosen ratio so subsequent guard checks exercise the
/// threshold deterministically. Pulls the `TableState` lock to mutate counters directly.
fn seed_ratio(encoder: &EncoderDynamicTable, bytes_in: u32, bytes_out: u32) {
    let mut state = encoder.state.lock().unwrap();
    state.bytes_in = bytes_in;
    state.bytes_out = bytes_out;
}

fn read_counters(encoder: &EncoderDynamicTable) -> (u32, u32) {
    let state = encoder.state.lock().unwrap();
    (state.bytes_in, state.bytes_out)
}

/// With the guard enabled and the running ratio already above threshold, a would-be
/// warming insert (Insert + literal) is suppressed — no encoder-stream op fires.
#[test]
fn guard_suppresses_warming_insert_when_ratio_above_threshold() {
    // Mnemonic on, zero blocking budget → warming-insert is the only indexing strategy.
    // Ratio threshold 0.5 so a modest seed trips it.
    let encoder = new_table_configured(4096, 0, true, 0.5);
    let _ = drain_instructions(&encoder); // drain the init SetDynamicTableCapacity.

    // First sighting of `(x-warm, v)` populates the predictor ring (remember runs at end
    // of plan_header_line). No encoder-stream op — first-sighting gets `None` allowance.
    encode_one(&encoder, "x-warm", "v", 1);
    let ops = drain_instructions(&encoder);
    assert!(ops.is_empty(), "first sighting should not emit an op");

    // Seed counters above threshold, then repeat — second sighting gets `Full` allowance
    // and, with no blocking budget, would pick the warming-insert program. The guard
    // should fire and suppress it.
    seed_ratio(&encoder, 1000, 800); // ratio 0.8 > 0.5
    encode_one(&encoder, "x-warm", "v", 3);

    let ops = drain_instructions(&encoder);
    assert!(
        ops.is_empty(),
        "guard should suppress warming insert, got {ops:?}",
    );
}

/// Guard disabled (`1.0`): warming insert fires even at a high running ratio — verifies
/// the test scaffolding exercises the warming-insert path and the suppression in the
/// previous test is truly attributable to the guard.
#[test]
fn guard_disabled_allows_warming_insert_at_high_ratio() {
    let encoder = new_table_configured(4096, 0, true, 1.0); // disabled
    let _ = drain_instructions(&encoder);

    encode_one(&encoder, "x-warm", "v", 1);
    let _ = drain_instructions(&encoder);

    seed_ratio(&encoder, 1000, 950); // ratio 0.95 — guard would fire if enabled
    encode_one(&encoder, "x-warm", "v", 3);

    let ops = drain_instructions(&encoder);
    assert!(
        !ops.is_empty(),
        "guard disabled: expected a warming-insert encoder-stream op",
    );
}

/// Insert-then-reference is exempt from the guard: the indexed ref in the same section
/// earns the encoder-stream bytes back, so even a high running ratio shouldn't suppress
/// it.
#[test]
fn guard_exempts_insert_then_reference() {
    // Non-zero blocking budget → predictor hit takes insert-then-reference, not warming.
    let encoder = new_table_configured(4096, 100, true, 0.5);
    let _ = drain_instructions(&encoder);

    encode_one(&encoder, "x-hot", "v", 1); // priming sighting.
    let _ = drain_instructions(&encoder);

    seed_ratio(&encoder, 1000, 800); // ratio 0.8 > 0.5

    encode_one(&encoder, "x-hot", "v", 3); // insert-then-reference — not double-lit.

    let ops = drain_instructions(&encoder);
    assert!(
        !ops.is_empty(),
        "insert-then-reference exempt from guard: expected an encoder-stream op",
    );
}

/// `add_compression_counters` with a large `bytes_out` triggers a rescale that preserves
/// the approximate ratio and resets `bytes_out` to 1000.
#[test]
fn counter_rescale_preserves_ratio() {
    let encoder = new_table_configured(4096, 0, true, 1.0);
    let _ = drain_instructions(&encoder);

    // Start near the rescale threshold; one more call pushes past it.
    let halfway = 1u32 << 30;
    seed_ratio(&encoder, halfway, halfway); // ratio = 1.0

    let push = (1u32 << 30) + 1000;
    encoder
        .state
        .lock()
        .unwrap()
        .add_compression_counters(push, push);

    let (bi, bo) = read_counters(&encoder);
    assert_eq!(bo, 1000, "bytes_out rescaled to 1000");
    assert!(
        (950..=1050).contains(&bi),
        "bytes_in should preserve ratio (~1000), got {bi}",
    );
}

/// Name-only insert (phase-4 step 4) is also a double-literal program: the Insert With
/// Literal Name goes to the encoder stream AND the section emits a literal. The guard
/// should suppress it on a high running ratio.
#[test]
fn guard_suppresses_name_only_insert() {
    let encoder = new_table_configured(4096, 100, true, 0.5);
    let _ = drain_instructions(&encoder);

    // First sighting of `(x-name-only, a)` populates the predictor.
    encode_one(&encoder, "x-name-only", "a", 1);
    let _ = drain_instructions(&encoder);

    seed_ratio(&encoder, 1000, 800); // ratio 0.8 > 0.5

    // Second encode, different value → predictor has name-seen but not nameval-seen for
    // this pair → `IndexingAllowance::NameOnly`. Table still empty so no dynamic name ref
    // is available — select_program would pick the name-only insert branch. Guard
    // suppresses.
    encode_one(&encoder, "x-name-only", "b", 3);

    let ops = drain_instructions(&encoder);
    assert!(
        ops.is_empty(),
        "guard should suppress name-only insert, got {ops:?}",
    );
}
