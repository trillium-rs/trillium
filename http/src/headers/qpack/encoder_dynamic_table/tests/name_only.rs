//! Phase-4 step-4 tests: the name-only insert strategy.
//!
//! Each test drives the mnemonic predictor into the `(name_seen, !nameval_seen)` state and
//! checks the resulting planner emission. The predictor is always on here because the
//! strategy fires only on the `IndexingAllowance::NameOnly` branch, which the predictor
//! gates.

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

#[test]
fn name_seen_without_pair_triggers_name_only_insert() {
    // Section 1 encodes (x-custom, v1) as a literal — first sighting, predictor records
    // `(name_hash, nameval_hash_v1)`. Section 2 encodes (x-custom, v2): `nameval_seen`
    // remains false (v2 is new), `name_seen` is true → `IndexingAllowance::NameOnly`.
    // With no full match, no static name match, no existing dynamic name match, and a
    // blocking slot available, the planner emits the name-only insert branch.
    let encoder = new_table_with_mnemonic(4096);
    let _ = drain_instructions(&encoder); // SetCapacity

    let mut h1 = Headers::new();
    h1.insert("x-custom", "v1");
    let _ = encode_headers(&encoder, PseudoHeaders::default(), &h1, 1);
    let _ = drain_instructions(&encoder); // first-sighting was literal; nothing expected

    let mut h2 = Headers::new();
    h2.insert("x-custom", "v2");
    let bytes = encode_headers(&encoder, PseudoHeaders::default(), &h2, 2);

    let (_prefix, lines) = parse_section(&bytes);
    let [FieldLineInstruction::LiteralDynamicNameRef { value, .. }] = lines.as_slice() else {
        panic!("expected LiteralDynamicNameRef referencing the new entry, got {lines:?}");
    };
    assert_eq!(value.as_bytes(), b"v2");

    // Encoder stream should carry an Insert With Literal Name whose value is empty.
    let instrs = drain_instructions(&encoder);
    assert!(
        matches!(
            instrs.as_slice(),
            [EncoderInstruction::InsertWithLiteralName { name, value }]
                if name == &qen("x-custom") && value.is_empty()
        ),
        "expected Insert With Literal Name with empty value, got {instrs:?}",
    );
    assert_eq!(encoder.insert_count(), 1);
}

#[test]
fn name_only_insert_skipped_when_static_name_match_available() {
    // `accept-encoding` is in the QPACK static table. Even if the predictor says NameOnly,
    // pick_literal_action's static-name-ref wins — the name-only insert branch is skipped
    // because a static name match is strictly cheaper (no encoder-stream cost).
    let encoder = new_table_with_mnemonic(4096);
    let _ = drain_instructions(&encoder);

    // Prime the predictor: encode (accept-encoding, v1) once so name_hash lands in the
    // ring. The nameval_hash for (accept-encoding, v1) ≠ (accept-encoding, v2).
    let mut h1 = Headers::new();
    h1.insert("accept-encoding", "v1");
    let _ = encode_headers(&encoder, PseudoHeaders::default(), &h1, 1);
    let _ = drain_instructions(&encoder);

    let mut h2 = Headers::new();
    h2.insert("accept-encoding", "v2");
    let bytes = encode_headers(&encoder, PseudoHeaders::default(), &h2, 2);

    let (_prefix, lines) = parse_section(&bytes);
    let [FieldLineInstruction::LiteralStaticNameRef { .. }] = lines.as_slice() else {
        panic!("expected LiteralStaticNameRef, got {lines:?}");
    };
    assert!(
        drain_instructions(&encoder).is_empty(),
        "name-only insert must not fire when a static name ref is available",
    );
    assert_eq!(encoder.insert_count(), 0);
}

#[test]
fn name_only_insert_skipped_when_dynamic_name_match_available() {
    // First cross-predictor section inserts the (name, "") entry via name-only. The
    // second cross-predictor section with yet another value finds the existing
    // dynamic-name match — name-only is skipped, pick_literal_action emits
    // LiteralDynamicNameRef against the existing entry, no new insert.
    let encoder = new_table_with_mnemonic(4096);
    let _ = drain_instructions(&encoder);

    // Prime predictor with v1 (literal, no insert).
    let mut h1 = Headers::new();
    h1.insert("x-custom", "v1");
    let _ = encode_headers(&encoder, PseudoHeaders::default(), &h1, 1);
    let _ = drain_instructions(&encoder);

    // v2 triggers name-only insert (predictor: NameOnly, no name refs, slot available).
    let mut h2 = Headers::new();
    h2.insert("x-custom", "v2");
    let _ = encode_headers(&encoder, PseudoHeaders::default(), &h2, 2);
    let _ = drain_instructions(&encoder); // drain the Insert
    assert_eq!(encoder.insert_count(), 1);

    // v3 — predictor still says NameOnly (nameval_hash for v3 not in ring; name_hash is).
    // An existing dynamic name entry now exists → name-only is skipped, we emit
    // LiteralDynamicNameRef against the existing entry with no new insert.
    let mut h3 = Headers::new();
    h3.insert("x-custom", "v3");
    let bytes = encode_headers(&encoder, PseudoHeaders::default(), &h3, 3);

    let (_prefix, lines) = parse_section(&bytes);
    let [FieldLineInstruction::LiteralDynamicNameRef { value, .. }] = lines.as_slice() else {
        panic!("expected LiteralDynamicNameRef against existing entry, got {lines:?}");
    };
    assert_eq!(value.as_bytes(), b"v3");

    assert!(
        drain_instructions(&encoder).is_empty(),
        "no new insert expected once a dynamic name ref exists",
    );
    assert_eq!(encoder.insert_count(), 1);
}

#[test]
fn name_only_insert_skipped_without_blocking_slot() {
    // Mnemonic on but max_blocked_streams = 0 → `can_take_blocking_slot()` always false
    // (no stream can become blocking). The name-only branch falls through to the literal
    // tail. Without a static or dynamic name match, the emission is literal-literal and
    // no encoder-stream op fires.
    let encoder = new_table_configured(4096, 0, true, 1.0);
    let _ = drain_instructions(&encoder);

    let mut h1 = Headers::new();
    h1.insert("x-custom", "v1");
    let _ = encode_headers(&encoder, PseudoHeaders::default(), &h1, 1);
    let _ = drain_instructions(&encoder);

    let mut h2 = Headers::new();
    h2.insert("x-custom", "v2");
    let bytes = encode_headers(&encoder, PseudoHeaders::default(), &h2, 2);

    let (_prefix, lines) = parse_section(&bytes);
    let [FieldLineInstruction::LiteralLiteralName { .. }] = lines.as_slice() else {
        panic!("expected LiteralLiteralName, got {lines:?}");
    };
    assert!(drain_instructions(&encoder).is_empty());
    assert_eq!(encoder.insert_count(), 0);
}
