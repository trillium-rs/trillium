use super::*;

#[test]
fn reverse_index_basic_lookup() {
    let table = new_table(4096);
    let abs = table.insert(qen("x-custom"), fv("hello")).unwrap();
    let name = qen("x-custom");
    assert_eq!(table.find_full_match(&name, b"hello"), Some(abs));
    assert_eq!(table.find_name_match(&name), Some(abs));
    assert_eq!(table.find_full_match(&name, b"other"), None);
    assert_eq!(table.find_name_match(&qen("x-missing")), None);
}

#[test]
fn reverse_index_duplicate_updates_to_newer_abs_idx() {
    let table = new_table(4096);
    table.insert(qen("x-custom"), fv("v")).unwrap();
    let second = table.insert(qen("x-custom"), fv("v")).unwrap();
    let name = qen("x-custom");
    // Both full-match and name-match should report the newer (higher) abs_idx.
    assert_eq!(table.find_full_match(&name, b"v"), Some(second));
    assert_eq!(table.find_name_match(&name), Some(second));
}

#[test]
fn reverse_index_staleness_preserves_newer_after_eviction() {
    // Capacity 68 fits exactly two entries of size 34. Insert (a,1), Duplicate it → two
    // entries with the same (name, value). Inserting (c,3) evicts the oldest (`a` at abs 0);
    // the per-value slot in the reverse index must NOT be cleared because a newer entry
    // still carries that value.
    let table = new_table(68);
    let first = table.insert(qen("a"), fv("1")).unwrap();
    let second = table.insert(qen("a"), fv("1")).unwrap(); // Duplicate wire form
    assert_ne!(first, second);
    table.insert(qen("c"), fv("3")).unwrap();
    assert_eq!(table.entry_count(), 2);

    let name_a = qen("a");
    assert_eq!(table.find_full_match(&name_a, b"1"), Some(second));
    assert_eq!(table.find_name_match(&name_a), Some(second));
}

#[test]
fn reverse_index_removes_name_when_all_values_evicted() {
    // Capacity 34 fits exactly one entry of size 34.
    let table = new_table(34);
    table.insert(qen("a"), fv("1")).unwrap();
    let b_abs = table.insert(qen("b"), fv("2")).unwrap();
    assert_eq!(table.entry_count(), 1);
    let name_a = qen("a");
    let name_b = qen("b");
    // "a" is fully evicted — neither lookup should find it.
    assert_eq!(table.find_full_match(&name_a, b"1"), None);
    assert_eq!(table.find_name_match(&name_a), None);
    // "b" is still live.
    assert_eq!(table.find_full_match(&name_b, b"2"), Some(b_abs));
    assert_eq!(table.find_name_match(&name_b), Some(b_abs));
}
