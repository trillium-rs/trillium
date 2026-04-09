use super::*;

#[test]
fn insert_literal_name_for_unknown_header_without_prior_entry() {
    let table = new_table(4096);
    let abs = table.insert(qen("x-custom"), fv("hello")).unwrap();
    assert_eq!(abs, 0);
    assert_eq!(table.insert_count(), 1);

    assert_eq!(
        drain_instructions(&table),
        vec![
            EncoderInstruction::SetCapacity(4096),
            EncoderInstruction::InsertWithLiteralName {
                name: qen("x-custom"),
                value: b"hello".to_vec(),
            },
        ]
    );
}

#[test]
fn insert_roundtrips_through_decoder_as_literal_name() {
    let table = new_table(4096);
    table.insert(qen("x-custom"), fv("hello")).unwrap();
    let decoder = apply_ops_to_decoder(&table, 4096);
    assert_eq!(decoder.name_at_relative(0).unwrap().as_ref(), "x-custom");
}

#[test]
fn insert_rejects_entry_exceeding_capacity() {
    let table = new_table(64);
    // entry_size = name + value + 32; a 64-byte value forces entry_size > 64.
    let big = fvo(vec![b'x'; 64]);
    assert!(table.insert(qen("n"), big).is_err());
}

#[test]
fn literal_name_inserts_evict_oldest_when_unpinned() {
    // Capacity 70 fits exactly two entries of size 34 (1 + 1 + 32). The third insert must
    // evict the oldest. None of these names have static slots, so each insert chooses the
    // literal-name form.
    let table = new_table(70);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    assert_eq!(table.entry_count(), 2);

    table.insert(qen("c"), fv("3")).unwrap();
    assert_eq!(table.insert_count(), 3);
    assert_eq!(table.entry_count(), 2);

    // Oldest ("a") is gone; newest two are "b" and "c".
    assert_eq!(table.find_name_match(&qen("a")), None);
    assert!(table.find_name_match(&qen("b")).is_some());
    assert!(table.find_name_match(&qen("c")).is_some());
}

#[test]
fn insert_known_header_with_static_slot_chooses_static_name_ref() {
    // `accept-encoding` has static table entries; a custom value triggers Insert With Name
    // Reference (T=1) rather than a literal name.
    let table = new_table(4096);
    table
        .insert(KnownHeaderName::AcceptEncoding.into(), fv("identity"))
        .unwrap();

    let instrs = drain_instructions(&table);
    assert!(matches!(
        instrs.as_slice(),
        [
            EncoderInstruction::SetCapacity(_),
            EncoderInstruction::InsertWithStaticNameRef { value, .. },
        ] if value == b"identity"
    ));
}

#[test]
fn insert_pseudo_header_with_static_slot_chooses_static_name_ref() {
    // `:path` has static slot 1 (`:path: /`). A different path value still uses the static
    // name reference.
    let table = new_table(4096);
    table.insert(qen(":path"), fv("/api/users")).unwrap();

    let instrs = drain_instructions(&table);
    assert!(matches!(
        instrs.as_slice(),
        [
            EncoderInstruction::SetCapacity(_),
            EncoderInstruction::InsertWithStaticNameRef {
                name_index: 1,
                value,
            },
        ] if value == b"/api/users"
    ));
}

#[test]
fn insert_pseudo_header_without_static_slot_uses_literal_name() {
    // `:protocol` has no static slot; the first insert lands as a literal name.
    let table = new_table(4096);
    table.insert(qen(":protocol"), fv("webtransport")).unwrap();

    let instrs = drain_instructions(&table);
    assert!(matches!(
        &instrs[..],
        [
            EncoderInstruction::SetCapacity(_),
            EncoderInstruction::InsertWithLiteralName { name, value },
        ] if name == &qen(":protocol") && value == b"webtransport"
    ));
}

#[test]
fn insert_second_value_for_unknown_name_chooses_dynamic_name_ref() {
    // `x-custom` has no static slot, so the second insert with the same name but a different
    // value falls through to dynamic-name-ref (T=0).
    let table = new_table(4096);
    table.insert(qen("x-custom"), fv("first")).unwrap();
    let abs = table.insert(qen("x-custom"), fv("second")).unwrap();
    assert_eq!(abs, 1);

    let instrs = drain_instructions(&table);
    assert_eq!(instrs.len(), 3);
    assert!(matches!(instrs[0], EncoderInstruction::SetCapacity(_)));
    assert!(matches!(
        instrs[1],
        EncoderInstruction::InsertWithLiteralName { .. }
    ));
    assert!(matches!(
        &instrs[2],
        EncoderInstruction::InsertWithDynamicNameRef { relative_index: 0, value }
            if value == b"second"
    ));
}

#[test]
fn dynamic_name_ref_insert_preserves_source_when_capacity_tight() {
    // Capacity 34 fits exactly one entry of size 34. The first entry has no static slot, so
    // the second insert (same name, different value) takes the dynamic-name-ref path — which
    // has to keep the source entry alive across eviction. No room → the insert must error.
    let table = new_table(34);
    table.insert(qen("a"), fv("1")).unwrap();
    assert!(table.insert(qen("a"), fv("2")).is_err());
    // Source entry is still there.
    assert_eq!(table.entry_count(), 1);
    assert_eq!(table.find_full_match(&qen("a"), b"1"), Some(0));
}

#[test]
fn second_insert_of_same_name_value_chooses_duplicate() {
    let table = new_table(4096);
    table.insert(qen("x-custom"), fv("hello")).unwrap();
    let second = table.insert(qen("x-custom"), fv("hello")).unwrap();
    assert_eq!(second, 1);
    assert_eq!(table.insert_count(), 2);
    assert_eq!(table.entry_count(), 2);

    let instrs = drain_instructions(&table);
    assert_eq!(instrs.len(), 3);
    assert!(matches!(
        instrs[2],
        EncoderInstruction::Duplicate { relative_index: 0 }
    ));
}

#[test]
fn duplicate_is_chosen_even_when_static_name_ref_is_available() {
    // `content-type` has a static name slot, but since the (name, value) pair already exists
    // in the dynamic table, Duplicate is strictly cheaper on the wire (no value bytes).
    let table = new_table(4096);
    table
        .insert(KnownHeaderName::ContentType.into(), fv("application/json"))
        .unwrap();
    table
        .insert(KnownHeaderName::ContentType.into(), fv("application/json"))
        .unwrap();

    let instrs = drain_instructions(&table);
    assert!(matches!(
        instrs[1],
        EncoderInstruction::InsertWithStaticNameRef { .. }
    ));
    assert!(matches!(
        instrs[2],
        EncoderInstruction::Duplicate { relative_index: 0 }
    ));
}

#[test]
fn duplicate_picks_matching_entry_across_multiple_names() {
    // Picker must match the full (name, value) pair, not just the name. Even though `a:1` at
    // abs 0 and `a:1` could in principle be duplicated, the intermediate `b:1` must not be.
    let table = new_table(4096);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("1")).unwrap();
    let third = table.insert(qen("a"), fv("1")).unwrap();
    assert_eq!(third, 2);

    let instrs = drain_instructions(&table);
    assert_eq!(instrs.len(), 4);
    // Last op must be a Duplicate. RFC §3.2.4 relative index = insert_count - 1 - abs_idx;
    // at the moment the duplicate is encoded, insert_count = 2 and the source abs_idx = 0,
    // so relative_index = 1.
    assert_eq!(
        instrs[3],
        EncoderInstruction::Duplicate { relative_index: 1 }
    );
}

#[test]
fn duplicate_source_remains_live_after_duplicate() {
    let table = new_table(4096);
    let first = table.insert(qen("x-custom"), fv("hello")).unwrap();
    let second = table.insert(qen("x-custom"), fv("hello")).unwrap();
    assert_eq!((first, second), (0, 1));
    assert_eq!(table.entry_count(), 2);

    // Both abs indices resolve, and the full-match lookup returns the newer one.
    assert_eq!(
        table.find_full_match(&qen("x-custom"), b"hello"),
        Some(second)
    );
}

#[test]
fn duplicate_fails_when_capacity_only_fits_one_entry() {
    // Capacity 34 fits exactly one entry of size 34. Duplicating the sole entry would need
    // to evict it first — but it's also the source we're copying from, so eviction is
    // blocked by the preserve-source floor.
    let table = new_table(34);
    table.insert(qen("a"), fv("1")).unwrap();
    assert!(table.insert(qen("a"), fv("1")).is_err());
    assert_eq!(table.entry_count(), 1);
}

#[test]
fn duplicate_succeeds_when_capacity_fits_two_entries() {
    // Capacity 68 fits two entries of size 34. Duplicating the sole entry succeeds.
    let table = new_table(68);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("a"), fv("1")).unwrap();
    assert_eq!(table.entry_count(), 2);

    let instrs = drain_instructions(&table);
    assert!(matches!(
        instrs[2],
        EncoderInstruction::Duplicate { relative_index: 0 }
    ));
}

#[test]
fn roundtrip_static_name_ref_insert() {
    let table = new_table(4096);
    table
        .insert(KnownHeaderName::AcceptEncoding.into(), fv("identity"))
        .unwrap();
    let decoder = apply_ops_to_decoder(&table, 4096);
    assert_eq!(
        decoder.name_at_relative(0).unwrap().as_ref(),
        "accept-encoding"
    );
}

#[test]
fn roundtrip_dynamic_name_ref_insert() {
    let table = new_table(4096);
    table.insert(qen("x-custom"), fv("first")).unwrap();
    table.insert(qen("x-custom"), fv("second")).unwrap();
    let decoder = apply_ops_to_decoder(&table, 4096);
    assert_eq!(decoder.name_at_relative(0).unwrap().as_ref(), "x-custom");
    assert_eq!(decoder.name_at_relative(1).unwrap().as_ref(), "x-custom");
}

#[test]
fn roundtrip_duplicate() {
    let table = new_table(4096);
    table.insert(qen("x-custom"), fv("hello")).unwrap();
    table.insert(qen("x-custom"), fv("hello")).unwrap();
    let decoder = apply_ops_to_decoder(&table, 4096);
    assert_eq!(decoder.name_at_relative(0).unwrap().as_ref(), "x-custom");
    assert_eq!(decoder.name_at_relative(1).unwrap().as_ref(), "x-custom");
}

#[test]
fn roundtrip_pseudo_header_without_static_slot() {
    // Covers the literal-name path for a pseudo-header (`:protocol`).
    let table = new_table(4096);
    table
        .insert(
            QpackEntryName::Pseudo(PseudoHeaderName::Protocol),
            fv("webtransport"),
        )
        .unwrap();
    let decoder = apply_ops_to_decoder(&table, 4096);
    assert_eq!(decoder.name_at_relative(0).unwrap().as_ref(), ":protocol");
}
