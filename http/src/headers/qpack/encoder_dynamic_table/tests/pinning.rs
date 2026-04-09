use super::*;

#[test]
fn pinned_entry_blocks_eviction() {
    // Capacity 70 fits two entries; the third insert would normally evict the oldest. Pin
    // abs index 0 and the insert must error.
    let table = new_table(70);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    table.register_outstanding_section(
        4,
        SectionRefs {
            required_insert_count: 1,
            min_ref_abs_idx: Some(0),
        },
    );
    assert!(table.insert(qen("c"), fv("3")).is_err());
    assert_eq!(table.entry_count(), 2);
}

#[test]
fn eviction_walks_past_older_unpinned_entries_to_reach_target() {
    // Pin abs 1 but leave abs 0 unpinned. A third insert needs to evict the oldest entry
    // to fit — abs 0 is free to go, and the pin at abs 1 protects only itself. Regression
    // guard against the historical `evicted_abs <= min_live` check, which over-protected
    // abs 0 (strictly older than the pin) and rejected otherwise-legal evictions.
    let table = new_table(70);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    table.register_outstanding_section(
        4,
        SectionRefs {
            required_insert_count: 2,
            min_ref_abs_idx: Some(1),
        },
    );
    assert!(table.insert(qen("c"), fv("3")).is_ok());
    // abs 0 was evicted; the pinned abs 1 and newly-inserted abs 2 remain.
    assert_eq!(table.entry_count(), 2);
}

#[test]
fn section_ack_advances_known_received() {
    let table = new_table(4096);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    table.register_outstanding_section(
        4,
        SectionRefs {
            required_insert_count: 2,
            min_ref_abs_idx: Some(0),
        },
    );
    table.on_section_ack(4).unwrap();
    assert_eq!(table.known_received_count(), 2);
}

#[test]
fn section_ack_without_outstanding_errors() {
    let table = EncoderDynamicTable::default();
    assert!(table.on_section_ack(4).is_err());
}

#[test]
fn stream_cancel_drops_sections_without_advancing() {
    let table = EncoderDynamicTable::default();
    table.register_outstanding_section(
        4,
        SectionRefs {
            required_insert_count: 3,
            min_ref_abs_idx: Some(0),
        },
    );
    table.on_stream_cancel(4);
    assert_eq!(table.known_received_count(), 0);
    assert!(table.state.lock().unwrap().outstanding_sections.is_empty());
}

#[test]
fn insert_count_increment_advances_and_bounds() {
    let table = new_table(4096);
    table.insert(qen("a"), fv("1")).unwrap();
    table.insert(qen("b"), fv("2")).unwrap();
    table.on_insert_count_increment(1).unwrap();
    assert_eq!(table.known_received_count(), 1);
    table.on_insert_count_increment(1).unwrap();
    assert_eq!(table.known_received_count(), 2);
    // Cannot advance past insert_count.
    assert!(table.on_insert_count_increment(1).is_err());
}

#[test]
fn insert_count_increment_rejects_zero() {
    let table = EncoderDynamicTable::default();
    assert!(table.on_insert_count_increment(0).is_err());
}

#[test]
fn fail_sets_failed_state() {
    let table = EncoderDynamicTable::default();
    table.fail(H3ErrorCode::QpackEncoderStreamError);
    assert_eq!(table.failed(), Some(H3ErrorCode::QpackEncoderStreamError));
}
