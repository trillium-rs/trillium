//! RFC 7541 Appendix C decoder test vectors.
//!
//! Covers §C.2 (literal representation cases), §C.3 (request sequence without Huffman),
//! §C.4 (same request sequence with Huffman), and §C.5 (response sequence exercising
//! eviction at a 256-byte table limit). §C.6 (Huffman-coded responses) is omitted — the
//! Huffman path is already validated by C.4, and the decoder wouldn't distinguish the
//! Huffman encoding from its §C.5 plain-text counterpart once strings are decoded.

use super::decode;
use crate::{
    KnownHeaderName,
    headers::{
        entry_name::EntryName, field_section::FieldLineValue, hpack::dynamic_table::DynamicTable,
    },
};

/// Default protocol max table size for the Appendix-C examples. C.1-C.4 are unconstrained;
/// C.5 specifies 256 octets.
const DEFAULT_MAX: usize = 4096;

/// Assert the decoder-side dynamic table matches the expected `(dyn_index, name, value)`
/// tuples. `dyn_index` is 1-based (1 = newest), matching the RFC's presentation.
fn assert_table(table: &DynamicTable, expected: &[(usize, &str, &[u8])]) {
    assert_eq!(
        table.len(),
        expected.len(),
        "dynamic table length mismatch — got {} entries, expected {}",
        table.len(),
        expected.len()
    );
    for (dyn_index, expected_name, expected_value) in expected {
        let entry = table
            .get(*dyn_index)
            .unwrap_or_else(|| panic!("no entry at dynamic index {dyn_index}"));
        assert_eq!(
            entry.name.as_str(),
            *expected_name,
            "entry {dyn_index} name mismatch"
        );
        assert_eq!(
            entry.value.as_bytes(),
            *expected_value,
            "entry {dyn_index} value mismatch"
        );
    }
}

fn assert_table_size(table: &DynamicTable, expected: usize) {
    assert_eq!(
        table.size(),
        expected,
        "dynamic table size accounting mismatch"
    );
}

/// C.2.1 — Literal Header Field with Incremental Indexing, literal name + literal value.
#[test]
fn c_2_1_literal_header_with_indexing() {
    const INPUT: &[u8] = &[
        0x40, 0x0a, 0x63, 0x75, 0x73, 0x74, 0x6f, 0x6d, 0x2d, 0x6b, 0x65, 0x79, 0x0d, 0x63, 0x75,
        0x73, 0x74, 0x6f, 0x6d, 0x2d, 0x68, 0x65, 0x61, 0x64, 0x65, 0x72,
    ];

    let mut table = DynamicTable::new(DEFAULT_MAX);
    let fs = decode(INPUT, &mut table, DEFAULT_MAX).unwrap();

    let (_, headers) = fs.into_parts();
    assert_eq!(
        headers.get_str("custom-key").map(ToOwned::to_owned),
        Some("custom-header".to_owned())
    );

    // "custom-key" = 10, "custom-header" = 13, overhead 32 → 55.
    assert_table(&table, &[(1, "custom-key", b"custom-header")]);
    assert_table_size(&table, 55);
}

/// C.2.2 — Literal Header Field without Indexing, indexed name + literal value.
#[test]
fn c_2_2_literal_header_without_indexing() {
    const INPUT: &[u8] = &[
        0x04, 0x0c, 0x2f, 0x73, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x2f, 0x70, 0x61, 0x74, 0x68,
    ];

    let mut table = DynamicTable::new(DEFAULT_MAX);
    let fs = decode(INPUT, &mut table, DEFAULT_MAX).unwrap();

    assert_eq!(fs.pseudo_headers().path(), Some("/sample/path"));
    assert_eq!(table.len(), 0, "without-indexing MUST NOT insert");
}

/// C.2.3 — Literal Header Field Never Indexed.
#[test]
fn c_2_3_literal_header_never_indexed() {
    const INPUT: &[u8] = &[
        0x10, 0x08, 0x70, 0x61, 0x73, 0x73, 0x77, 0x6f, 0x72, 0x64, 0x06, 0x73, 0x65, 0x63, 0x72,
        0x65, 0x74,
    ];

    let mut table = DynamicTable::new(DEFAULT_MAX);
    let fs = decode(INPUT, &mut table, DEFAULT_MAX).unwrap();

    let (_, headers) = fs.into_parts();
    assert_eq!(
        headers.get_str("password").map(ToOwned::to_owned),
        Some("secret".to_owned())
    );
    assert_eq!(table.len(), 0, "never-indexed MUST NOT insert");
}

/// C.2.4 — Indexed Header Field (static entry).
#[test]
fn c_2_4_indexed_header() {
    const INPUT: &[u8] = &[0x82];

    let mut table = DynamicTable::new(DEFAULT_MAX);
    let fs = decode(INPUT, &mut table, DEFAULT_MAX).unwrap();

    assert_eq!(
        fs.pseudo_headers().method(),
        Some(crate::Method::Get),
        "static index 2 = :method: GET"
    );
    assert_eq!(table.len(), 0);
}

// ---- C.3 Request Examples without Huffman Coding ----

const C3_REQUEST_1: &[u8] = &[
    0x82, 0x86, 0x84, 0x41, 0x0f, 0x77, 0x77, 0x77, 0x2e, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65,
    0x2e, 0x63, 0x6f, 0x6d,
];

const C3_REQUEST_2: &[u8] = &[
    0x82, 0x86, 0x84, 0xbe, 0x58, 0x08, 0x6e, 0x6f, 0x2d, 0x63, 0x61, 0x63, 0x68, 0x65,
];

const C3_REQUEST_3: &[u8] = &[
    0x82, 0x87, 0x85, 0xbf, 0x40, 0x0a, 0x63, 0x75, 0x73, 0x74, 0x6f, 0x6d, 0x2d, 0x6b, 0x65, 0x79,
    0x0c, 0x63, 0x75, 0x73, 0x74, 0x6f, 0x6d, 0x2d, 0x76, 0x61, 0x6c, 0x75, 0x65,
];

#[test]
fn c_3_request_sequence_without_huffman() {
    let mut table = DynamicTable::new(DEFAULT_MAX);

    // C.3.1
    let fs = decode(C3_REQUEST_1, &mut table, DEFAULT_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().method(), Some(crate::Method::Get));
    assert_eq!(fs.pseudo_headers().scheme(), Some("http"));
    assert_eq!(fs.pseudo_headers().path(), Some("/"));
    assert_eq!(fs.pseudo_headers().authority(), Some("www.example.com"));
    assert_table(&table, &[(1, ":authority", b"www.example.com")]);
    assert_table_size(&table, 57);

    // C.3.2
    let fs = decode(C3_REQUEST_2, &mut table, DEFAULT_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().method(), Some(crate::Method::Get));
    assert_eq!(fs.pseudo_headers().scheme(), Some("http"));
    assert_eq!(fs.pseudo_headers().path(), Some("/"));
    assert_eq!(fs.pseudo_headers().authority(), Some("www.example.com"));
    let (_, headers) = fs.into_parts();
    assert_eq!(
        headers
            .get_str(KnownHeaderName::CacheControl)
            .map(ToOwned::to_owned),
        Some("no-cache".to_owned())
    );
    assert_table(
        &table,
        &[
            (1, "cache-control", b"no-cache"),
            (2, ":authority", b"www.example.com"),
        ],
    );
    assert_table_size(&table, 110);

    // C.3.3
    let fs = decode(C3_REQUEST_3, &mut table, DEFAULT_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().method(), Some(crate::Method::Get));
    assert_eq!(fs.pseudo_headers().scheme(), Some("https"));
    assert_eq!(fs.pseudo_headers().path(), Some("/index.html"));
    assert_eq!(fs.pseudo_headers().authority(), Some("www.example.com"));
    let (_, headers) = fs.into_parts();
    assert_eq!(
        headers.get_str("custom-key").map(ToOwned::to_owned),
        Some("custom-value".to_owned())
    );
    assert_table(
        &table,
        &[
            (1, "custom-key", b"custom-value"),
            (2, "cache-control", b"no-cache"),
            (3, ":authority", b"www.example.com"),
        ],
    );
    assert_table_size(&table, 164);
}

// ---- C.4 Request Examples with Huffman Coding ----

const C4_REQUEST_1: &[u8] = &[
    0x82, 0x86, 0x84, 0x41, 0x8c, 0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90, 0xf4,
    0xff,
];

const C4_REQUEST_2: &[u8] = &[
    0x82, 0x86, 0x84, 0xbe, 0x58, 0x86, 0xa8, 0xeb, 0x10, 0x64, 0x9c, 0xbf,
];

const C4_REQUEST_3: &[u8] = &[
    0x82, 0x87, 0x85, 0xbf, 0x40, 0x88, 0x25, 0xa8, 0x49, 0xe9, 0x5b, 0xa9, 0x7d, 0x7f, 0x89, 0x25,
    0xa8, 0x49, 0xe9, 0x5b, 0xb8, 0xe8, 0xb4, 0xbf,
];

#[test]
fn c_4_request_sequence_with_huffman() {
    let mut table = DynamicTable::new(DEFAULT_MAX);

    let fs = decode(C4_REQUEST_1, &mut table, DEFAULT_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().authority(), Some("www.example.com"));
    assert_table_size(&table, 57);

    let fs = decode(C4_REQUEST_2, &mut table, DEFAULT_MAX).unwrap();
    let (_, headers) = fs.into_parts();
    assert_eq!(
        headers
            .get_str(KnownHeaderName::CacheControl)
            .map(ToOwned::to_owned),
        Some("no-cache".to_owned())
    );
    assert_table_size(&table, 110);

    let fs = decode(C4_REQUEST_3, &mut table, DEFAULT_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().path(), Some("/index.html"));
    let (_, headers) = fs.into_parts();
    assert_eq!(
        headers.get_str("custom-key").map(ToOwned::to_owned),
        Some("custom-value".to_owned())
    );
    assert_table_size(&table, 164);
}

// ---- C.5 Response Examples without Huffman Coding (256-byte table, eviction) ----

const C5_MAX: usize = 256;

const C5_RESPONSE_1: &[u8] = &[
    0x48, 0x03, 0x33, 0x30, 0x32, 0x58, 0x07, 0x70, 0x72, 0x69, 0x76, 0x61, 0x74, 0x65, 0x61, 0x1d,
    0x4d, 0x6f, 0x6e, 0x2c, 0x20, 0x32, 0x31, 0x20, 0x4f, 0x63, 0x74, 0x20, 0x32, 0x30, 0x31, 0x33,
    0x20, 0x32, 0x30, 0x3a, 0x31, 0x33, 0x3a, 0x32, 0x31, 0x20, 0x47, 0x4d, 0x54, 0x6e, 0x17, 0x68,
    0x74, 0x74, 0x70, 0x73, 0x3a, 0x2f, 0x2f, 0x77, 0x77, 0x77, 0x2e, 0x65, 0x78, 0x61, 0x6d, 0x70,
    0x6c, 0x65, 0x2e, 0x63, 0x6f, 0x6d,
];

const C5_RESPONSE_2: &[u8] = &[0x48, 0x03, 0x33, 0x30, 0x37, 0xc1, 0xc0, 0xbf];

const C5_RESPONSE_3: &[u8] = &[
    0x88, 0xc1, 0x61, 0x1d, 0x4d, 0x6f, 0x6e, 0x2c, 0x20, 0x32, 0x31, 0x20, 0x4f, 0x63, 0x74, 0x20,
    0x32, 0x30, 0x31, 0x33, 0x20, 0x32, 0x30, 0x3a, 0x31, 0x33, 0x3a, 0x32, 0x32, 0x20, 0x47, 0x4d,
    0x54, 0xc0, 0x5a, 0x04, 0x67, 0x7a, 0x69, 0x70, 0x77, 0x38, 0x66, 0x6f, 0x6f, 0x3d, 0x41, 0x53,
    0x44, 0x4a, 0x4b, 0x48, 0x51, 0x4b, 0x42, 0x5a, 0x58, 0x4f, 0x51, 0x57, 0x45, 0x4f, 0x50, 0x49,
    0x55, 0x41, 0x58, 0x51, 0x57, 0x45, 0x4f, 0x49, 0x55, 0x3b, 0x20, 0x6d, 0x61, 0x78, 0x2d, 0x61,
    0x67, 0x65, 0x3d, 0x33, 0x36, 0x30, 0x30, 0x3b, 0x20, 0x76, 0x65, 0x72, 0x73, 0x69, 0x6f, 0x6e,
    0x3d, 0x31,
];

#[test]
fn c_5_response_sequence_with_eviction() {
    let mut table = DynamicTable::new(C5_MAX);

    // C.5.1 — all four fields are inserted (via incremental indexing); status=302
    // occupies 42 bytes. Total = 222, fits under 256.
    let fs = decode(C5_RESPONSE_1, &mut table, C5_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().status(), Some(crate::Status::Found)); // 302
    assert_table(
        &table,
        &[
            (1, "location", b"https://www.example.com"),
            (2, "date", b"Mon, 21 Oct 2013 20:13:21 GMT"),
            (3, "cache-control", b"private"),
            (4, ":status", b"302"),
        ],
    );
    assert_table_size(&table, 222);

    // C.5.2 — :status:307 insert (42 bytes) forces eviction of :status:302 (42 bytes).
    // Three Indexed-Add references to previous dynamic entries follow (no inserts).
    let fs = decode(C5_RESPONSE_2, &mut table, C5_MAX).unwrap();
    assert_eq!(
        fs.pseudo_headers().status(),
        Some(crate::Status::TemporaryRedirect)
    );
    let (_, headers) = fs.into_parts();
    assert_eq!(
        headers
            .get_str(KnownHeaderName::CacheControl)
            .map(ToOwned::to_owned),
        Some("private".to_owned())
    );
    assert_table(
        &table,
        &[
            (1, ":status", b"307"),
            (2, "location", b"https://www.example.com"),
            (3, "date", b"Mon, 21 Oct 2013 20:13:21 GMT"),
            (4, "cache-control", b"private"),
        ],
    );
    assert_table_size(&table, 222);

    // C.5.3 — inserts :status:200, references cache-control, then a new date (evicts
    // cache-control), references location, then inserts content-encoding (evicts the old
    // date), then inserts a huge set-cookie value (evicts location and :status:307).
    let fs = decode(C5_RESPONSE_3, &mut table, C5_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().status(), Some(crate::Status::Ok));
    let (_, headers) = fs.into_parts();
    assert_eq!(
        headers
            .get_str(KnownHeaderName::ContentEncoding)
            .map(ToOwned::to_owned),
        Some("gzip".to_owned())
    );
    assert_eq!(
        headers
            .get_str(KnownHeaderName::SetCookie)
            .map(ToOwned::to_owned),
        Some("foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1".to_owned())
    );
    assert_table(
        &table,
        &[
            (
                1,
                "set-cookie",
                b"foo=ASDJKHQKBZXOQWEOPIUAXQWEOIU; max-age=3600; version=1",
            ),
            (2, "content-encoding", b"gzip"),
            (3, "date", b"Mon, 21 Oct 2013 20:13:22 GMT"),
        ],
    );
    assert_table_size(&table, 215);
}

/// Sanity: `FieldLineValue` for decoded dynamic-table entries should be Owned (came from the
/// wire, not static string slices).
#[test]
fn inserted_values_are_owned() {
    let mut table = DynamicTable::new(DEFAULT_MAX);
    decode(C3_REQUEST_1, &mut table, DEFAULT_MAX).unwrap();
    let entry = table.get(1).unwrap();
    assert!(matches!(entry.value, FieldLineValue::Owned(_)));
    assert!(matches!(entry.name, EntryName::Pseudo(_)));
}
