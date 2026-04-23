use super::*;
use crate::{
    Headers, KnownHeaderName, Method,
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
fn static_full_match_preferred_over_dynamic() {
    // When the same `(name, value)` pair is in both the static table and the dynamic
    // table, the encoder picks the static reference. Static refs cost no RIC, no pinning,
    // no blocked-stream slot — strictly cheaper than the equivalent dynamic ref. The
    // dynamic copy of a static-matchable pair only ever exists in pathological cases
    // (something inserted it directly); the planner's own warm-insert path short-circuits
    // on static_full and never adds such an entry.
    let encoder = new_table(4096);
    encoder
        .insert(KnownHeaderName::ContentType.into(), fv("application/json"))
        .unwrap();
    encoder.on_insert_count_increment(1).unwrap();

    let mut headers = Headers::new();
    headers.insert(KnownHeaderName::ContentType, "application/json");
    let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

    let (prefix, lines) = parse_section(&bytes);
    assert_eq!(prefix.encoded_required_insert_count, 0);
    assert!(
        matches!(
            lines.as_slice(),
            [FieldLineInstruction::IndexedStatic { .. }]
        ),
        "expected IndexedStatic, got {lines:?}"
    );
}

#[test]
fn dynamic_full_match_pseudo_method() {
    // `:method PATCH` has no static full match (only name ref to 15). Seed the dynamic
    // table so a later encode finds a full dynamic match.
    let encoder = new_table(4096);
    let abs = encoder
        .insert(EntryName::Pseudo(PseudoHeaderName::Method), fv("PATCH"))
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
            EntryName::Pseudo(PseudoHeaderName::Protocol),
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
