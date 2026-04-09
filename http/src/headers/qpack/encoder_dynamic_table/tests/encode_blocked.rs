use super::*;
use crate::{
    Headers,
    headers::qpack::{
        FieldSection, PseudoHeaders,
        instruction::field_section::{FieldLineInstruction, FieldSectionPrefix},
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
