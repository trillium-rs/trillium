//! Phase-3 tests: the mnemonic predictor's effect on planner decisions.
//!
//! Isolated from the phase-2 mechanism tests in `encode_refs.rs` et al. by using
//! [`new_table_with_mnemonic`] instead of [`new_table`]. The predictor suppresses first
//! sightings from indexing; only the second-plus sighting of a `(name, value)` pair within
//! the recent-history window earns an insert.

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

#[test]
fn first_sighting_is_not_indexed() {
    // Mnemonic on + fresh `(name, value)` + no budget pressure → predictor says "not seen"
    // → `allow_indexing = false` → eager insert-then-reference does not fire. The header
    // emits as a literal and no encoder-stream op is produced.
    let encoder = new_table_with_mnemonic(4096);
    let _ = drain_instructions(&encoder); // drain SetCapacity

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
        "expected literal-with-literal-name, got {lines:?}",
    );
    assert_eq!(
        encoder.insert_count(),
        0,
        "no dynamic-table insert expected"
    );
    assert!(
        drain_instructions(&encoder).is_empty(),
        "no encoder-stream op expected on first sighting",
    );
}

#[test]
fn second_sighting_triggers_insert_then_reference() {
    // Same header encoded twice across two sections. First section: literal (predictor
    // miss). Second section: predictor hits → insert-then-reference fires.
    let encoder = new_table_with_mnemonic(4096);
    let _ = drain_instructions(&encoder);

    let mut headers = Headers::new();
    headers.insert("x-custom", "hello");

    // Section 1 — first sighting: literal, no insert.
    let bytes1 = encode(&encoder, PseudoHeaders::default(), &headers, 1);
    let (p1, lines1) = parse_section(&bytes1);
    assert_eq!(p1, FieldSectionPrefix::default(), "section 1 RIC=0");
    assert!(matches!(
        lines1.as_slice(),
        [FieldLineInstruction::LiteralLiteralName { .. }]
    ));
    assert_eq!(encoder.insert_count(), 0);

    // Section 2 — second sighting: predictor hits, eager insert-then-reference fires.
    let bytes2 = encode(&encoder, PseudoHeaders::default(), &headers, 2);
    let (p2, lines2) = parse_section(&bytes2);
    assert_ne!(
        p2.encoded_required_insert_count, 0,
        "section 2 must reference the newly-inserted entry"
    );
    assert!(
        matches!(
            lines2.as_slice(),
            [FieldLineInstruction::IndexedDynamic { .. }]
        ),
        "expected indexed-dynamic reference, got {lines2:?}",
    );
    assert_eq!(encoder.insert_count(), 1);

    // Encoder stream carried the Insert.
    let instrs = drain_instructions(&encoder);
    assert!(
        matches!(
            instrs.as_slice(),
            [EncoderInstruction::InsertWithLiteralName { name, value }]
                if name == &qen("x-custom") && value == b"hello"
        ),
        "expected Insert With Literal Name on 2nd sighting, got {instrs:?}",
    );
}

#[test]
fn second_sighting_same_section_triggers_insert_and_full_match() {
    // Two occurrences of `x-custom: hello` within a single section. The predictor sees
    // the second occurrence as "seen", so that line takes insert-then-reference; the
    // following `x-custom: hello` in the same section is handled by the dynamic full
    // match path.
    //
    // `Headers::append` preserves insertion order for repeat names under the same key,
    // so both occurrences land in the field-lines list in order.
    let encoder = new_table_with_mnemonic(4096);
    let _ = drain_instructions(&encoder);

    let mut headers = Headers::new();
    headers.append("x-custom", "hello");
    headers.append("x-custom", "hello");

    let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);
    let (prefix, lines) = parse_section(&bytes);
    assert_ne!(prefix.encoded_required_insert_count, 0);

    // First field line: literal (predictor miss). Second: indexed-dynamic (insert fired
    // on the 2nd occurrence's predictor hit, then the planner referenced the fresh entry).
    //
    // Order-sensitive because `append` under the same name preserves order.
    let [first, second] = lines.as_slice() else {
        panic!("expected 2 field lines, got {lines:?}");
    };
    assert!(
        matches!(first, FieldLineInstruction::LiteralLiteralName { .. }),
        "first line should be a literal (predictor miss), got {first:?}",
    );
    assert!(
        matches!(second, FieldLineInstruction::IndexedDynamic { .. }),
        "second line should be indexed-dynamic (predictor hit + insert + ref), got {second:?}",
    );
    assert_eq!(encoder.insert_count(), 1);
}

#[test]
fn mnemonic_off_indexes_eagerly_on_first_sighting() {
    // Control: with mnemonic off AND a blocking-streams budget available, the phase-2
    // eager path fires on first sighting — a regression guard for the config knob.
    let encoder = new_table_with_blocked_streams(4096, 100);
    let _ = drain_instructions(&encoder);

    let mut headers = Headers::new();
    headers.insert("x-custom", "hello");
    let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

    let (prefix, lines) = parse_section(&bytes);
    assert_ne!(
        prefix.encoded_required_insert_count, 0,
        "mnemonic-off must index on first sighting",
    );
    assert!(matches!(
        lines.as_slice(),
        [FieldLineInstruction::IndexedDynamic { .. }]
    ));
    assert_eq!(encoder.insert_count(), 1);
}

#[test]
fn sensitive_header_skips_predictor_and_is_not_indexed() {
    // `authorization` is sensitive — the sensitive-header veto takes precedence over the
    // predictor. Even encoded twice it must never produce an insert.
    let encoder = new_table_with_mnemonic(4096);
    let _ = drain_instructions(&encoder);

    let mut headers = Headers::new();
    headers.insert("authorization", "Bearer secret");

    for stream_id in 1..=2 {
        let bytes = encode(&encoder, PseudoHeaders::default(), &headers, stream_id);
        let (prefix, lines) = parse_section(&bytes);
        assert_eq!(prefix, FieldSectionPrefix::default());
        assert!(matches!(
            lines.as_slice(),
            [FieldLineInstruction::LiteralStaticNameRef { .. }]
        ));
    }
    assert_eq!(encoder.insert_count(), 0);
    assert!(drain_instructions(&encoder).is_empty());
}

#[test]
fn distinct_values_under_same_name_each_need_two_sightings() {
    // Two different values under the same name are independent in the predictor. Each
    // needs its own second sighting to earn an insert.
    let encoder = new_table_with_mnemonic(4096);
    let _ = drain_instructions(&encoder);

    // Two sections of `x-custom: foo`, two sections of `x-custom: bar`. After all four,
    // we expect exactly two inserts (the 2nd foo and the 2nd bar).
    for (stream_id, value) in [(1, "foo"), (2, "foo"), (3, "bar"), (4, "bar")] {
        let mut headers = Headers::new();
        headers.insert("x-custom", value);
        let _ = encode(&encoder, PseudoHeaders::default(), &headers, stream_id);
    }

    assert_eq!(
        encoder.insert_count(),
        2,
        "expected one insert per distinct (name, value) pair on its 2nd sighting",
    );
}
