use super::*;

// ---- Blocked streams budget (§4.5.1.2) ----

#[test]
fn blocked_streams_empty_budget_available() {
    let table = new_table_with_blocked_streams(4096, 2);
    assert_eq!(table.currently_blocked_streams(), 0);
    assert!(!table.is_stream_blocking(1));
    assert!(table.can_block_another_stream(1));
}

#[test]
fn blocked_streams_non_blocking_section_does_not_count() {
    // RIC == 0 is a static-only section and never blocks, even when KRC is also 0.
    let table = new_table_with_blocked_streams(4096, 0);
    table.register_outstanding_section(1, blocking_section(0));
    assert_eq!(table.currently_blocked_streams(), 0);
    assert!(!table.is_stream_blocking(1));
}

#[test]
fn blocked_streams_counts_distinct_streams() {
    let table = new_table_with_blocked_streams(4096, 3);
    table.register_outstanding_section(1, blocking_section(5));
    table.register_outstanding_section(2, blocking_section(7));
    assert_eq!(table.currently_blocked_streams(), 2);
    assert!(table.is_stream_blocking(1));
    assert!(table.is_stream_blocking(2));
    assert!(!table.is_stream_blocking(3));
}

#[test]
fn blocked_streams_per_stream_dedup() {
    // Three sections on one stream — all blocking, but it's still one blocked stream.
    let table = new_table_with_blocked_streams(4096, 1);
    table.register_outstanding_section(1, blocking_section(3));
    table.register_outstanding_section(1, blocking_section(5));
    table.register_outstanding_section(1, blocking_section(7));
    assert_eq!(table.currently_blocked_streams(), 1);
    assert!(table.can_block_another_stream(1)); // already blocking, free
    assert!(!table.can_block_another_stream(2)); // would exceed budget of 1
}

#[test]
fn blocked_streams_budget_exhausted() {
    let table = new_table_with_blocked_streams(4096, 2);
    table.register_outstanding_section(1, blocking_section(5));
    table.register_outstanding_section(2, blocking_section(7));
    assert_eq!(table.currently_blocked_streams(), 2);
    assert!(table.can_block_another_stream(1)); // already blocking
    assert!(table.can_block_another_stream(2)); // already blocking
    assert!(!table.can_block_another_stream(3)); // new stream, budget full
}

// ---- Required Insert Count encoding (§4.5.1.1) and Set Dynamic Table Capacity (§3.2.1) ----

#[test]
fn encode_required_insert_count_formula() {
    // ric == 0 always encodes as 0 regardless of capacity.
    assert_eq!(encode_required_insert_count(0, 0), 0);
    assert_eq!(encode_required_insert_count(0, 4096), 0);

    // max_capacity 32 → max_entries 1, full_range 2.
    assert_eq!(encode_required_insert_count(1, 32), 2); // (1 % 2) + 1
    assert_eq!(encode_required_insert_count(2, 32), 1); // (2 % 2) + 1
    assert_eq!(encode_required_insert_count(3, 32), 2);

    // max_capacity 100 → max_entries 3, full_range 6.
    assert_eq!(encode_required_insert_count(5, 100), 6); // (5 % 6) + 1
    assert_eq!(encode_required_insert_count(6, 100), 1); // (6 % 6) + 1
    assert_eq!(encode_required_insert_count(13, 100), 2); // (13 % 6) + 1

    // max_capacity 4096 → max_entries 128, full_range 256.
    assert_eq!(encode_required_insert_count(1, 4096), 2);
    assert_eq!(encode_required_insert_count(256, 4096), 1);
    assert_eq!(encode_required_insert_count(300, 4096), 45); // (300 % 256) + 1
}

#[test]
fn set_capacity_emits_wire_instruction() {
    let table = new_table(4096);
    table.set_capacity(1024).unwrap();
    assert_eq!(
        drain_instructions(&table),
        vec![
            EncoderInstruction::SetCapacity(4096),
            EncoderInstruction::SetCapacity(1024),
        ]
    );
}

#[test]
fn set_capacity_rejects_above_max() {
    let table = new_table(4096);
    assert!(table.set_capacity(8192).is_err());
}

#[test]
fn drain_is_fifo() {
    // Initialization emitted SetCapacity(4096); two more to verify FIFO ordering on the wire.
    let table = new_table(4096);
    table.set_capacity(256).unwrap();
    table.set_capacity(1024).unwrap();
    assert_eq!(
        drain_instructions(&table),
        vec![
            EncoderInstruction::SetCapacity(4096),
            EncoderInstruction::SetCapacity(256),
            EncoderInstruction::SetCapacity(1024),
        ]
    );
}
