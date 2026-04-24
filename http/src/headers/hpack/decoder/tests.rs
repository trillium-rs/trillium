//! Smoke tests for the HPACK decoder.
//!
//! Focused unit tests; the full RFC 7541 Appendix C vector sweep lives in a separate test
//! file landing with the encoder + integration tests.

use super::decode;
use crate::{
    KnownHeaderName, Method, Status,
    headers::{
        compression_error::CompressionError,
        entry_name::{EntryName, PseudoHeaderName},
        field_section::{FieldLineValue, PseudoHeaders},
        hpack::{HpackDecodeError, dynamic_table::DynamicTable},
    },
};

const DEFAULT_PROTOCOL_MAX: usize = 4096;

fn fresh_table() -> DynamicTable {
    DynamicTable::new(DEFAULT_PROTOCOL_MAX)
}

/// §6.1 — the byte `0x82` is Indexed Header Field representation of static index 2,
/// i.e. `:method: GET`.
#[test]
fn indexed_static_method_get() {
    let mut table = fresh_table();
    let fs = decode(&[0x82], &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().method(), Some(Method::Get));
}

/// §6.1 — `0x86` → static index 6 → `:scheme: http`.
#[test]
fn indexed_static_scheme_http() {
    let mut table = fresh_table();
    let fs = decode(&[0x86], &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().scheme(), Some("http"));
}

/// §6.1 — index 0 MUST be treated as a decoding error (§6.1 / §2.3.3).
#[test]
fn indexed_zero_is_error() {
    let mut table = fresh_table();
    assert!(matches!(
        decode(&[0x80], &mut table, DEFAULT_PROTOCOL_MAX),
        Err(HpackDecodeError::Compression(
            CompressionError::InvalidStaticIndex(0)
        ))
    ));
}

/// §6.1 — dynamic index past the end of the table is a decoding error.
#[test]
fn indexed_beyond_dynamic_is_error() {
    let mut table = fresh_table();
    // Index 62 (first dynamic slot) while the table is empty.
    // Encoded as the single byte 0b1_0111110 = 0xBE.
    assert!(matches!(
        decode(&[0xBE], &mut table, DEFAULT_PROTOCOL_MAX),
        Err(HpackDecodeError::Compression(
            CompressionError::InvalidStaticIndex(62)
        ))
    ));
}

/// §6.2.2 — Literal Header Field without Indexing, with static name reference (:authority)
/// and literal value "www.example.com". This is the middle of RFC 7541 C.2.2.
#[test]
fn literal_without_indexing_static_name_ref() {
    // 0x04 = 0000_0100 = literal-without-indexing, name-index=4 (:path in static table),
    // but let me use :authority to match intent: index 1 → 0x01.
    // Bytes: 0x01 | len=15 | "www.example.com"
    let mut bytes = vec![0x01, 15];
    bytes.extend_from_slice(b"www.example.com");

    let mut table = fresh_table();
    let fs = decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().authority(), Some("www.example.com"));
    // §6.2.2 never inserts into the dynamic table.
    assert_eq!(table.len(), 0);
}

/// §6.2.1 — Literal Header Field with Incremental Indexing, with literal name and value.
/// Taken from RFC 7541 C.2.1: `custom-key: custom-header`.
#[test]
fn literal_with_indexing_literal_name() {
    // 0x40 = 0100_0000 = literal-with-indexing, index=0 (literal name follows)
    // Name: len=10, "custom-key"
    // Value: len=13, "custom-header"
    let mut bytes = vec![0x40, 10];
    bytes.extend_from_slice(b"custom-key");
    bytes.push(13);
    bytes.extend_from_slice(b"custom-header");

    let mut table = fresh_table();
    let fs = decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();

    let (pseudos, headers) = fs.into_parts();
    assert_eq!(pseudos, PseudoHeaders::default());
    assert_eq!(
        headers.get_str("custom-key").map(ToOwned::to_owned),
        Some("custom-header".to_owned())
    );

    // §6.2.1 MUST insert into the dynamic table.
    assert_eq!(table.len(), 1);
    let entry = table.get(1).unwrap();
    assert!(matches!(
        entry.name,
        EntryName::Unknown(_) | EntryName::Known(_)
    ));
    assert_eq!(entry.value.as_bytes(), b"custom-header");
}

/// §6.2.1 followed by §6.1 indexed dynamic reference. The second representation pulls the
/// just-inserted entry out by HPACK absolute index 62.
#[test]
fn insert_then_reference_dynamic_entry() {
    // First: literal-with-indexing + literal name "x-foo": "bar"
    let mut bytes = vec![0x40, 5];
    bytes.extend_from_slice(b"x-foo");
    bytes.push(3);
    bytes.extend_from_slice(b"bar");
    // Second: indexed-header field at absolute index 62 (first dynamic slot)
    // Encoded: 0b1_0111110 = 0xBE.
    bytes.push(0xBE);

    let mut table = fresh_table();
    let fs = decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    let (_, headers) = fs.into_parts();
    let values: Vec<_> = headers
        .get_values("x-foo")
        .unwrap()
        .into_iter()
        .map(|v| v.as_ref().to_vec())
        .collect();
    assert_eq!(values, vec![b"bar".to_vec(), b"bar".to_vec()]);
}

/// §6.3 — a size update at the start of a block shrinks the dynamic table.
#[test]
fn size_update_shrinks_table() {
    let mut table = fresh_table();
    // Pre-populate: "x-foo: bar" inserted via incremental indexing.
    let mut seed = vec![0x40, 5];
    seed.extend_from_slice(b"x-foo");
    seed.push(3);
    seed.extend_from_slice(b"bar");
    decode(&seed, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    assert_eq!(table.len(), 1);

    // Now send a size update to 0, which must evict everything.
    // 0b001_00000 = 0x20 is "size update, value=0".
    let block = [0x20];
    decode(&block, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    assert_eq!(table.len(), 0);
}

/// §6.3 — a size update larger than the protocol limit MUST be treated as a decoding error.
#[test]
fn size_update_above_protocol_max_is_error() {
    let mut table = fresh_table();
    // 0b001_11111 = 0x3F, then varint continuation for (128 - 31) = 97, then terminator.
    // Actually simpler: encode size=10000 with 5-bit prefix — runs over for values > 30.
    // Use: 0x3F (prefix max 31), then 10000 - 31 = 9969 in continuation bytes.
    // Let's use a smaller oversize: size update = 32 with protocol max = 16.
    let block = [0x20 | 16]; // size update value=16, fits prefix
    let result = decode(&block, &mut table, 15);
    assert!(matches!(
        result,
        Err(HpackDecodeError::Compression(
            CompressionError::InvalidStaticIndex(16)
        ))
    ));
}

/// §4.2 — a size update after a field representation MUST be a decoding error.
#[test]
fn size_update_after_field_is_error() {
    let mut table = fresh_table();
    // First: indexed header (:method GET = 0x82)
    // Then: size update (0x20)
    let block = [0x82, 0x20];
    assert!(matches!(
        decode(&block, &mut table, DEFAULT_PROTOCOL_MAX),
        Err(HpackDecodeError::Compression(
            CompressionError::UnexpectedEnd
        ))
    ));
}

/// Status pseudo-header via indexed static entry (0x88 = index 8 = :status 200).
#[test]
fn indexed_static_status_ok() {
    let mut table = fresh_table();
    let fs = decode(&[0x88], &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().status(), Some(Status::Ok));
}

/// Regular (non-pseudo) header inserted via literal without indexing, using the
/// accept-encoding static slot (index 16 has a non-empty value "gzip, deflate").
/// We reference it purely for the name here, and supply an override value.
#[test]
fn literal_overrides_static_slot_value() {
    // 0x10 = 0001_0000 = literal-never-indexed, index=0 (literal name follows)
    // But we want static name ref index 16 → 0001_xxxx with index=16.
    // 16 > 15 (4-bit max), so encoded as 0x1F (prefix all-ones) + 16 - 15 = 1
    // which is [0x1F, 0x01].
    let mut bytes = vec![0x1F, 0x01];
    bytes.push(2);
    bytes.extend_from_slice(b"br");

    let mut table = fresh_table();
    let fs = decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    let (_, headers) = fs.into_parts();
    assert_eq!(
        headers
            .get(KnownHeaderName::AcceptEncoding)
            .map(|v| v.as_ref().to_vec()),
        Some(b"br".to_vec())
    );
    // Never-indexed MUST NOT insert.
    assert_eq!(table.len(), 0);
}

/// RFC 9113 §8.1.2.3: duplicated pseudo-header fields are a malformed request. The decoder
/// surfaces this via [`HpackDecodeError::MalformedRequest`] so the caller can respond with
/// a stream-level `PROTOCOL_ERROR` without tearing down the connection.
#[test]
fn duplicate_pseudo_is_malformed() {
    let mut table = fresh_table();
    // :method GET (0x82) then :method POST (0x83).
    assert!(matches!(
        decode(&[0x82, 0x83], &mut table, DEFAULT_PROTOCOL_MAX),
        Err(HpackDecodeError::MalformedRequest(
            super::MalformedRequest::DuplicatePseudoHeader
        ))
    ));
}

/// RFC 9113 §8.1.2.1: a pseudo-header appearing after any regular header is a malformed
/// request.
#[test]
fn pseudo_after_regular_is_malformed() {
    let mut table = fresh_table();
    // :method GET (0x82 — §6.1 indexed static) → regular "foo: bar" (§6.2.2 literal
    // without indexing, new name) → :path / (0x85 — §6.1 indexed static).
    let mut bytes = vec![0x82, 0x00, 0x03];
    bytes.extend_from_slice(b"foo");
    bytes.push(0x03);
    bytes.extend_from_slice(b"bar");
    bytes.push(0x85);
    assert!(matches!(
        decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX),
        Err(HpackDecodeError::MalformedRequest(
            super::MalformedRequest::PseudoHeaderAfterRegular
        ))
    ));
}

/// Confirming the Pseudo field value pipeline: ensure Authority is surfaced as a str.
#[test]
fn pseudo_authority_surfaces() {
    let _ = PseudoHeaderName::Authority; // keep import live
    let mut table = fresh_table();
    let mut bytes = vec![0x01, 11]; // literal w/o indexing, :authority name ref
    bytes.extend_from_slice(b"example.com");
    let fs = decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().authority(), Some("example.com"));
}

/// Provenance: decoded values are Owned (came from the wire, materialized as Vec).
#[test]
fn decoded_values_are_owned() {
    let mut table = fresh_table();
    let mut bytes = vec![0x40, 3];
    bytes.extend_from_slice(b"foo");
    bytes.push(3);
    bytes.extend_from_slice(b"bar");
    decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    let entry = table.get(1).unwrap();
    assert!(matches!(entry.value, FieldLineValue::Owned(_)));
}
