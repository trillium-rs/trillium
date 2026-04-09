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
