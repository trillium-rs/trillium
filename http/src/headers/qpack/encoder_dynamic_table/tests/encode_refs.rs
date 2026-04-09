use super::*;
use crate::{
    Headers, KnownHeaderName,
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

fn roundtrip(
    encoder: &EncoderDynamicTable,
    decoder: &DecoderDynamicTable,
    pseudo: PseudoHeaders<'_>,
    headers: &Headers,
    stream_id: u64,
) -> FieldSection<'static> {
    let bytes = encode(encoder, pseudo, headers, stream_id);
    let enc_ops: Vec<u8> = encoder.drain_pending_ops().into_iter().flatten().collect();
    let mut enc_stream = &enc_ops[..];
    block_on(decoder.run_reader(&mut enc_stream)).unwrap();
    block_on(decoder.decode(&bytes, stream_id)).unwrap()
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

// ---- dynamic name reference (literal with dynamic name ref, §4.5.4 T=0) ----

#[test]
fn dynamic_name_ref_header_with_different_value() {
    let encoder = new_table(4096);
    encoder.insert(qen("x-custom"), fv("hello")).unwrap();
    encoder.on_insert_count_increment(1).unwrap();

    let mut headers = Headers::new();
    headers.insert("x-custom", "world");
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
    let [
        FieldLineInstruction::LiteralDynamicNameRef {
            relative_index: 0,
            value,
            never_indexed: false,
        },
    ] = lines.as_slice()
    else {
        panic!("expected literal-with-dynamic-name-ref, got {lines:?}");
    };
    assert_eq!(value.as_bytes(), b"world");

    let os = outstanding(&encoder, 1);
    assert_eq!(os.len(), 1);
    assert_eq!(os[0].required_insert_count, 1);
    assert_eq!(os[0].min_ref_abs_idx, Some(0));
}

#[test]
fn budget_zero_warming_inserts_when_no_full_match() {
    // KRC=0 and budget=0 mean any dynamic reference would block. With no full match
    // available, the warming-insert branch fires: the new entry is added to the encoder
    // stream now (referenceable in *future* sections after the peer acks) and this
    // section emits a literal — costing nothing toward RIC or the blocked-streams budget.
    let encoder = new_table_with_blocked_streams(4096, 0);
    encoder.insert(qen("x-custom"), fv("hello")).unwrap();
    // Drain the priming Insert so we can isolate the warming-insert op.
    let _ = drain_instructions(&encoder);

    let mut headers = Headers::new();
    headers.insert("x-custom", "world");
    // First sighting populates the recent-pairs ring; on its own this section emits a
    // literal with no encoder-stream activity (predictor hasn't seen the pair yet).
    let _ = encode(&encoder, PseudoHeaders::default(), &headers, 1);
    let _ = drain_instructions(&encoder);
    // Second sighting trips the predictor → warming insert.
    let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 3);

    let (prefix, lines) = parse_section(&bytes);
    assert_eq!(
        prefix,
        FieldSectionPrefix::default(),
        "warming insert must not register a RIC for this section",
    );
    assert!(
        matches!(
            lines.as_slice(),
            [FieldLineInstruction::LiteralLiteralName { .. }]
        ),
        "expected literal-with-literal-name in the section, got {lines:?}",
    );
    assert!(
        outstanding(&encoder, 1).is_empty(),
        "warming insert must not register an outstanding section",
    );

    // The encoder stream got the warming Insert — `x-custom` had no static slot but did
    // have a dynamic name match, so the smart-pick chose Insert With Name Reference T=0.
    let instrs = drain_instructions(&encoder);
    assert!(
        matches!(
            instrs.as_slice(),
            [EncoderInstruction::InsertWithDynamicNameRef { value, .. }] if value == b"world"
        ),
        "expected warming Insert With Name Reference T=0 for x-custom: world, got {instrs:?}",
    );
}

#[test]
fn budget_zero_warming_inserts_unknown_header_with_no_name_match() {
    // Empty table, budget=0, header has no dynamic name match and no static slot.
    // Warming insert: encoder stream gets Insert With Literal Name; section emits a
    // literal-with-literal-name.
    let encoder = new_table_with_blocked_streams(4096, 0);
    let _ = drain_instructions(&encoder); // drain SetCapacity

    let mut headers = Headers::new();
    headers.insert("x-fresh", "v1");
    // First sighting populates the recent-pairs ring; emission is a plain literal.
    let _ = encode(&encoder, PseudoHeaders::default(), &headers, 1);
    let _ = drain_instructions(&encoder);
    // Second sighting trips the predictor → warming insert.
    let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 3);

    let (prefix, lines) = parse_section(&bytes);
    assert_eq!(prefix, FieldSectionPrefix::default());
    assert!(matches!(
        lines.as_slice(),
        [FieldLineInstruction::LiteralLiteralName { .. }]
    ));
    assert!(outstanding(&encoder, 1).is_empty());

    let instrs = drain_instructions(&encoder);
    assert!(
        matches!(
            instrs.as_slice(),
            [EncoderInstruction::InsertWithLiteralName { name, value }]
                if name == &qen("x-fresh") && value == b"v1"
        ),
        "expected warming Insert With Literal Name, got {instrs:?}",
    );
}

#[test]
fn budget_zero_warming_inserts_known_header_uses_static_name_ref() {
    // Empty table, budget=0, header has a static name slot but no full match.
    // Warming insert: encoder stream gets Insert With Name Reference T=1 (static);
    // section emits literal-with-static-name-ref.
    let encoder = new_table_with_blocked_streams(4096, 0);
    let _ = drain_instructions(&encoder);

    let mut headers = Headers::new();
    headers.insert("content-type", "application/x-trillium-test");
    // First sighting populates the recent-pairs ring; emission is a plain literal-with-
    // static-name-ref (no warming insert yet).
    let _ = encode(&encoder, PseudoHeaders::default(), &headers, 1);
    let _ = drain_instructions(&encoder);
    // Second sighting trips the predictor → warming insert.
    let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 3);

    let (prefix, lines) = parse_section(&bytes);
    assert_eq!(prefix, FieldSectionPrefix::default());
    assert!(
        matches!(
            lines.as_slice(),
            [FieldLineInstruction::LiteralStaticNameRef { .. }]
        ),
        "expected literal-with-static-name-ref, got {lines:?}",
    );
    assert!(outstanding(&encoder, 1).is_empty());

    let instrs = drain_instructions(&encoder);
    assert!(
        matches!(
            instrs.as_slice(),
            [EncoderInstruction::InsertWithStaticNameRef { value, .. }]
                if value == b"application/x-trillium-test"
        ),
        "expected warming Insert With Static Name Reference, got {instrs:?}",
    );
}

#[test]
fn warming_insert_referenceable_in_next_section_after_ack() {
    // Section 1 populates the predictor ring. Section 2 trips it and warming-inserts
    // x-fresh: v1. After we apply the encoder stream and ack the entry (advances KRC),
    // section 3 references it non-blockingly even though the blocked-streams budget
    // remains zero.
    let encoder = new_table_with_blocked_streams(4096, 0);
    let _ = drain_instructions(&encoder);

    let mut h1 = Headers::new();
    h1.insert("x-fresh", "v1");

    // Section 1: predictor records, no warming insert yet.
    let bytes1 = encode(&encoder, PseudoHeaders::default(), &h1, 1);
    let (p1, _) = parse_section(&bytes1);
    assert_eq!(
        p1,
        FieldSectionPrefix::default(),
        "section 1 should not RIC"
    );
    let _ = drain_instructions(&encoder);

    // Section 2: predictor hits, warming insert fires; section emits a literal.
    let bytes2 = encode(&encoder, PseudoHeaders::default(), &h1, 3);
    let (p2, _) = parse_section(&bytes2);
    assert_eq!(
        p2,
        FieldSectionPrefix::default(),
        "section 2 should not RIC"
    );
    // Drain the warming Insert and pretend the peer acked it (advances KRC).
    let _ = drain_instructions(&encoder);
    encoder.on_insert_count_increment(1).unwrap();

    // Section 3: full match with KRC=1 → non-blocking IndexedDynamic.
    let bytes3 = encode(&encoder, PseudoHeaders::default(), &h1, 5);
    let (p3, lines3) = parse_section(&bytes3);
    assert_ne!(
        p3.encoded_required_insert_count, 0,
        "section 3 should reference the warmed entry"
    );
    assert!(
        matches!(
            lines3.as_slice(),
            [FieldLineInstruction::IndexedDynamic { .. }]
        ),
        "expected indexed dynamic ref, got {lines3:?}",
    );
}

#[test]
fn sensitive_header_does_not_warming_insert() {
    // `authorization` is on the sensitive-header skip list (the current stand-in for the
    // RFC 9204 §4.5.4 N bit). Even with budget=0 and a fresh header, the warming-insert
    // branch must not fire — emit a literal with no encoder-stream activity.
    let encoder = new_table_with_blocked_streams(4096, 0);
    let _ = drain_instructions(&encoder);

    let mut headers = Headers::new();
    headers.insert("authorization", "Bearer secret");
    let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

    let (prefix, lines) = parse_section(&bytes);
    assert_eq!(prefix, FieldSectionPrefix::default());
    assert!(matches!(
        lines.as_slice(),
        [FieldLineInstruction::LiteralStaticNameRef { .. }]
    ));
    assert!(outstanding(&encoder, 1).is_empty());

    let instrs = drain_instructions(&encoder);
    assert!(
        instrs.is_empty(),
        "sensitive-header skip must not produce a warming insert, got {instrs:?}",
    );
}

#[test]
fn dynamic_name_ref_already_blocking_stream_allows_ref() {
    // Capacity fits exactly one entry, so insert-then-reference cannot run (would evict
    // the pinned source). Falls through to dynamic name-ref.
    let encoder = new_table_with_blocked_streams(45, 1); // 8 + 5 + 32
    encoder.insert(qen("x-custom"), fv("hello")).unwrap();
    encoder.register_outstanding_section(
        1,
        SectionRefs {
            required_insert_count: 1,
            min_ref_abs_idx: Some(0),
        },
    );

    let mut headers = Headers::new();
    headers.insert("x-custom", "world");
    let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

    let (prefix, lines) = parse_section(&bytes);
    assert_ne!(prefix.encoded_required_insert_count, 0);
    assert!(
        matches!(
            lines.as_slice(),
            [FieldLineInstruction::LiteralDynamicNameRef { .. }]
        ),
        "expected literal-with-dynamic-name-ref, got {lines:?}",
    );
}

#[test]
fn static_name_ref_preferred_over_dynamic_name_ref() {
    let encoder = new_table(4096);
    encoder
        .insert(KnownHeaderName::ContentType.into(), fv("application/json"))
        .unwrap();
    encoder.on_insert_count_increment(1).unwrap();

    let mut headers = Headers::new();
    headers.insert(KnownHeaderName::ContentType, "application/xml");
    let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

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
        panic!("expected literal-with-static-name-ref, got {lines:?}");
    };
    assert_eq!(value.as_bytes(), b"application/xml");
    assert!(outstanding(&encoder, 1).is_empty());
}

#[test]
fn dynamic_name_ref_protocol_pseudo() {
    let encoder = new_table(4096);
    encoder
        .insert(
            QpackEntryName::Pseudo(PseudoHeaderName::Protocol),
            fv("webtransport"),
        )
        .unwrap();
    encoder.on_insert_count_increment(1).unwrap();

    let bytes = encode(
        &encoder,
        PseudoHeaders {
            protocol: Some(Cow::Borrowed("connect-udp")),
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
    let [
        FieldLineInstruction::LiteralDynamicNameRef {
            relative_index: 0,
            value,
            never_indexed: false,
        },
    ] = lines.as_slice()
    else {
        panic!("expected literal-with-dynamic-name-ref, got {lines:?}");
    };
    assert_eq!(value.as_bytes(), b"connect-udp");
}

#[test]
fn committed_to_blocking_covers_full_match_then_name_ref() {
    // Capacity for exactly two entries. Encoding order is forced (x-a first, then x-b) so
    // that by the time x-b's insert is attempted, the section has already pinned abs 0 —
    // the insert's eviction attempt hits the pin and falls through to dynamic name-ref.
    //
    // Uses `encode_field_lines` directly rather than `FieldSection::new` + `Headers` to
    // sidestep the HashMap-iteration non-determinism: the reverse order (x-b first) would
    // legitimately evict the unpinned x-a and produce different (but also correct) wire
    // bytes, defeating the specific assertion this test wants to make.
    let encoder = new_table_with_blocked_streams(72, 1); // 2 × (3 + 1 + 32)
    encoder.insert(qen("x-a"), fv("1")).unwrap();
    encoder.insert(qen("x-b"), fv("1")).unwrap();

    let field_lines = [(qen("x-a"), fv("1")), (qen("x-b"), fv("2"))];
    let mut bytes = Vec::new();
    encoder.encode_field_lines(&field_lines, &mut bytes, 1);

    let (prefix, lines) = parse_section(&bytes);
    // max_capacity=72 → max_entries=2; RIC=2 → encoded_ric = (2 % 4) + 1 = 3.
    assert_eq!(prefix.encoded_required_insert_count, 3);

    let [
        FieldLineInstruction::IndexedDynamic { .. },
        FieldLineInstruction::LiteralDynamicNameRef { .. },
    ] = lines.as_slice()
    else {
        panic!("expected indexed-dynamic + literal-dynamic-name-ref, got {lines:?}");
    };

    let os = outstanding(&encoder, 1);
    assert_eq!(os.len(), 1);
    assert_eq!(os[0].required_insert_count, 2);
}

#[test]
fn roundtrip_dynamic_name_ref_through_reader() {
    let encoder = new_table(4096);
    // The encoder's init SetCapacity will be drained into the decoder below via
    // `roundtrip`, so the decoder is constructed uninitialized.
    let decoder = DecoderDynamicTable::new(4096, 0);

    encoder.insert(qen("x-custom"), fv("hello")).unwrap();
    encoder.on_insert_count_increment(1).unwrap();

    let mut headers = Headers::new();
    headers.insert("x-custom", "world");
    let decoded = roundtrip(&encoder, &decoder, PseudoHeaders::default(), &headers, 1);
    let (_, decoded_headers) = decoded.into_parts();
    assert_eq!(decoded_headers.get_str("x-custom"), Some("world"));
}

// ---- multi-value and duplicate-value headers ----

#[test]
fn multi_value_header_emits_one_field_line_per_value() {
    let encoder = new_table(4096);
    let mut headers = Headers::new();
    headers.append("x-multi", "first");
    headers.append("x-multi", "second");
    let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

    let decoder = DecoderDynamicTable::new(4096, 0);
    let decoded = block_on(decoder.decode(&bytes, 1)).unwrap();
    let (_, decoded_headers) = decoded.into_parts();
    let values: Vec<&str> = decoded_headers
        .get_values("x-multi")
        .map(|vs| vs.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    assert_eq!(values, vec!["first", "second"]);
}

#[test]
fn section_prefix_encodes_nonzero_ric_with_delta_base_zero() {
    let encoder = new_table(4096);
    for i in 0..5u32 {
        encoder
            .insert(qen("x-custom"), fvo(i.to_string().into_bytes()))
            .unwrap();
    }
    encoder.on_insert_count_increment(5).unwrap();

    let mut headers = Headers::new();
    headers.insert("x-custom", "4");
    let bytes = encode(&encoder, PseudoHeaders::default(), &headers, 1);

    let (prefix, _) = parse_section(&bytes);
    // max_entries = 4096 / 32 = 128, so encoded_ric = (5 % 256) + 1 = 6.
    assert_eq!(
        prefix,
        FieldSectionPrefix {
            encoded_required_insert_count: 6,
            base_is_negative: false,
            delta_base: 0,
        }
    );
}

#[test]
fn roundtrip_dynamic_full_match_through_reader() {
    let encoder = new_table(4096);
    // The encoder's init SetCapacity will be drained into the decoder below via
    // `roundtrip`, so the decoder is constructed uninitialized.
    let decoder = DecoderDynamicTable::new(4096, 0);

    encoder.insert(qen("x-custom"), fv("hello")).unwrap();
    encoder.on_insert_count_increment(1).unwrap();

    let mut headers = Headers::new();
    headers.insert("x-custom", "hello");
    let decoded = roundtrip(&encoder, &decoder, PseudoHeaders::default(), &headers, 1);
    let (_, decoded_headers) = decoded.into_parts();
    assert_eq!(decoded_headers.get_str("x-custom"), Some("hello"));
}
