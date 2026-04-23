//! Encoder unit tests and HPACK encoder ↔ decoder round-trip.

use super::encode;
use crate::{
    Headers, KnownHeaderName, Method, Status,
    headers::{
        field_section::{FieldSection, PseudoHeaders},
        hpack::{decoder::decode, dynamic_table::DynamicTable},
    },
};

const DEFAULT_PROTOCOL_MAX: usize = 4096;

fn encode_field_section(pseudos: PseudoHeaders<'_>, headers: &Headers) -> Vec<u8> {
    let fs = FieldSection::new(pseudos, headers);
    let mut buf = Vec::new();
    encode(&fs, &mut buf);
    buf
}

/// `:method GET` is a full match against static index 2; encodes as the single byte 0x82.
#[test]
fn indexed_full_match_method_get() {
    let pseudos = PseudoHeaders::default().with_method(Method::Get);
    let bytes = encode_field_section(pseudos, &Headers::new());
    assert_eq!(bytes, &[0x82]);
}

/// `:scheme https` → single byte 0x87 (static index 7).
#[test]
fn indexed_full_match_scheme_https() {
    let pseudos = PseudoHeaders::default().with_scheme("https");
    let bytes = encode_field_section(pseudos, &Headers::new());
    assert_eq!(bytes, &[0x87]);
}

/// `:status 200` → single byte 0x88 (static index 8).
#[test]
fn indexed_full_match_status_ok() {
    let pseudos = PseudoHeaders::default().with_status(Status::Ok);
    let bytes = encode_field_section(pseudos, &Headers::new());
    assert_eq!(bytes, &[0x88]);
}

/// `:path /foo` → name match on static index 4 (":path"), literal value "/foo".
/// Encoding: 0x04 | len | "/foo" — but Huffman may win. Just verify round-trip.
#[test]
fn name_match_path_custom_value() {
    let pseudos = PseudoHeaders::default().with_path("/foo");
    let bytes = encode_field_section(pseudos, &Headers::new());
    assert!(!bytes.is_empty());

    // First byte starts 0000xxxx (literal w/o indexing); low 4 bits = 4 → 0x04.
    assert_eq!(bytes[0], 0x04);

    // Round-trip confirms value preserved.
    let mut table = DynamicTable::new(DEFAULT_PROTOCOL_MAX);
    let fs = decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().path(), Some("/foo"));
}

/// Custom (unknown) header name + value: no static match, so literal-name + literal-value.
/// Encoding starts with 0x00.
#[test]
fn no_match_literal_name_value() {
    let mut headers = Headers::new();
    headers.append("x-custom", "hello");
    let bytes = encode_field_section(PseudoHeaders::default(), &headers);
    assert_eq!(bytes[0], 0x00);

    // Round-trip.
    let mut table = DynamicTable::new(DEFAULT_PROTOCOL_MAX);
    let fs = decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    let (_, decoded) = fs.into_parts();
    assert_eq!(
        decoded.get_str("x-custom").map(ToOwned::to_owned),
        Some("hello".to_owned())
    );
}

/// A known header with a non-matching value should use `NameMatch`.
/// `content-type: application/json` — name-match on static index 31, literal value.
#[test]
fn name_match_known_header() {
    let mut headers = Headers::new();
    headers.append(KnownHeaderName::ContentType, "application/json");
    let bytes = encode_field_section(PseudoHeaders::default(), &headers);

    // 0x1F = 0000_1111: literal without indexing, index 15+ in 4-bit prefix.
    // For index 31, that's 0x0F + (31 - 15) = 0x0F, 0x10 → [0x0F, 0x10, ...].
    assert_eq!(bytes[0], 0x0F, "top 4 bits zero (literal-w/o-indexing)");
    assert_eq!(bytes[1], 0x10, "remainder 16 for index 31 via 4-bit prefix");

    // Round-trip.
    let mut table = DynamicTable::new(DEFAULT_PROTOCOL_MAX);
    let fs = decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    let (_, decoded) = fs.into_parts();
    assert_eq!(
        decoded
            .get_str(KnownHeaderName::ContentType)
            .map(ToOwned::to_owned),
        Some("application/json".to_owned())
    );
}

/// Full round-trip on a realistic request field section.
#[test]
fn round_trip_request() {
    let pseudos = PseudoHeaders::default()
        .with_method(Method::Get)
        .with_scheme("https")
        .with_path("/resource")
        .with_authority("example.com");

    let mut headers = Headers::new();
    headers.append(KnownHeaderName::UserAgent, "trillium-h2-test");
    headers.append(KnownHeaderName::Accept, "*/*");

    let bytes = encode_field_section(pseudos.clone(), &headers);

    let mut table = DynamicTable::new(DEFAULT_PROTOCOL_MAX);
    let fs = decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    assert_eq!(*fs.pseudo_headers(), pseudos);
    let (_, decoded_headers) = fs.into_parts();
    assert_eq!(
        decoded_headers
            .get_str(KnownHeaderName::UserAgent)
            .map(ToOwned::to_owned),
        Some("trillium-h2-test".to_owned())
    );
    assert_eq!(
        decoded_headers
            .get_str(KnownHeaderName::Accept)
            .map(ToOwned::to_owned),
        Some("*/*".to_owned())
    );
}

/// Huffman should be chosen when it produces a shorter encoding. "www.example.com" is the
/// classic RFC 7541 C.4 example — Huffman shrinks it from 15 to 12 bytes.
#[test]
fn huffman_chosen_when_shorter() {
    let pseudos = PseudoHeaders::default().with_authority("www.example.com");
    let bytes = encode_field_section(pseudos, &Headers::new());

    // Authority is static name-ref index 1 → literal-w/o-indexing encoded as 0x01.
    assert_eq!(bytes[0], 0x01);
    // Next byte is the string-literal header: H flag (0x80) set + length (7-bit prefix).
    assert_ne!(
        bytes[1] & 0x80,
        0,
        "Huffman flag must be set for shorter form"
    );

    // Round-trip.
    let mut table = DynamicTable::new(DEFAULT_PROTOCOL_MAX);
    let fs = decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    assert_eq!(fs.pseudo_headers().authority(), Some("www.example.com"));
}

/// Static-only encoder never inserts into the dynamic table, so a round-trip leaves the
/// decoder-side table empty.
#[test]
fn encoder_never_inserts_into_dynamic_table() {
    let mut headers = Headers::new();
    headers.append("x-custom", "hello");
    let bytes = encode_field_section(PseudoHeaders::default(), &headers);

    let mut table = DynamicTable::new(DEFAULT_PROTOCOL_MAX);
    decode(&bytes, &mut table, DEFAULT_PROTOCOL_MAX).unwrap();
    assert_eq!(
        table.len(),
        0,
        "static-only encoder shouldn't produce §6.2.1"
    );
}
