use super::PseudoHeaders;
use crate::{HeaderValue, Headers, KnownHeaderName, Method, Status, headers::qpack::FieldSection};
use std::borrow::Cow;

fn roundtrip(
    pseudo_headers: PseudoHeaders<'_>,
    headers: &Headers,
) -> (PseudoHeaders<'static>, Headers) {
    let mut buf = Vec::new();
    FieldSection::new(pseudo_headers, headers).encode(&mut buf);
    FieldSection::decode(&buf)
        .expect("decode failed")
        .into_parts()
}

#[test]
fn roundtrip_all_static_matches() {
    let mut headers = Headers::new();
    headers.insert(KnownHeaderName::AcceptEncoding, "gzip, deflate, br");

    let (pseudos, decoded_headers) = roundtrip(
        PseudoHeaders {
            method: Some(Method::Get),
            path: Some(Cow::Borrowed("/")),
            scheme: Some(Cow::Borrowed("https")),
            ..Default::default()
        },
        &headers,
    );

    assert_eq!(pseudos.method, Some(Method::Get));
    assert_eq!(pseudos.path.as_deref(), Some("/"));
    assert_eq!(pseudos.scheme.as_deref(), Some("https"));
    assert_eq!(
        decoded_headers.get_str(KnownHeaderName::AcceptEncoding),
        Some("gzip, deflate, br"),
    );
}

#[test]
fn roundtrip_name_ref_with_custom_value() {
    let mut headers = Headers::new();
    headers.insert(KnownHeaderName::ContentType, "application/xml");
    headers.insert(KnownHeaderName::Server, "trillium");

    let (pseudos, decoded_headers) = roundtrip(
        PseudoHeaders {
            status: Some(Status::Ok),
            ..Default::default()
        },
        &headers,
    );

    assert_eq!(pseudos.status, Some(Status::Ok));
    assert_eq!(
        decoded_headers.get_str(KnownHeaderName::ContentType),
        Some("application/xml"),
    );
    assert_eq!(
        decoded_headers.get_str(KnownHeaderName::Server),
        Some("trillium"),
    );
}

#[test]
fn roundtrip_literal_name_and_value() {
    let mut headers = Headers::new();
    headers.insert("x-custom-header", "custom-value");

    let (pseudos, decoded_headers) = roundtrip(
        PseudoHeaders {
            method: Some(Method::Post),
            ..Default::default()
        },
        &headers,
    );

    assert_eq!(pseudos.method, Some(Method::Post));
    assert_eq!(
        decoded_headers.get_str("x-custom-header"),
        Some("custom-value"),
    );
}

#[test]
fn roundtrip_mixed() {
    let mut headers = Headers::new();
    headers.insert(KnownHeaderName::AcceptRanges, "bytes");
    headers.insert(KnownHeaderName::CacheControl, "private");
    headers.insert("x-request-id", "abc-123");

    let (pseudos, decoded_headers) = roundtrip(
        PseudoHeaders {
            method: Some(Method::Get),
            path: Some(Cow::Borrowed("/")),
            authority: Some(Cow::Borrowed("example.com")),
            ..Default::default()
        },
        &headers,
    );

    assert_eq!(pseudos.method, Some(Method::Get));
    assert_eq!(pseudos.path.as_deref(), Some("/"));
    assert_eq!(pseudos.authority.as_deref(), Some("example.com"));
    assert_eq!(
        decoded_headers.get_str(KnownHeaderName::AcceptRanges),
        Some("bytes"),
    );
    assert_eq!(
        decoded_headers.get_str(KnownHeaderName::CacheControl),
        Some("private"),
    );
    assert_eq!(decoded_headers.get_str("x-request-id"), Some("abc-123"));
}

// RFC 9204 §B.1 — Literal Field Line with Name Reference
// :path = /index.html (name ref to static table index 1, no huffman)
#[test]
fn rfc9204_b1_decode() {
    let bytes = &[
        0x00, 0x00, // prefix: required insert count=0, delta base=0
        0x51, 0x0b, 0x2f, 0x69, 0x6e, 0x64, 0x65, 0x78, 0x2e, 0x68, 0x74, 0x6d, 0x6c,
    ];

    let (pseudos, headers) = FieldSection::decode(bytes)
        .expect("decode failed")
        .into_parts();
    assert_eq!(pseudos.path.as_deref(), Some("/index.html"));
    assert!(headers.is_empty());

    // Verify round-trip through our encoder
    let (pseudos2, _) = roundtrip(
        PseudoHeaders {
            path: Some(Cow::Borrowed("/index.html")),
            ..Default::default()
        },
        &Headers::new(),
    );
    assert_eq!(pseudos2.path.as_deref(), Some("/index.html"));
}

#[test]
fn roundtrip_all_static_table_methods() {
    let headers = Headers::new();
    for method in [
        Method::Connect,
        Method::Delete,
        Method::Get,
        Method::Head,
        Method::Options,
        Method::Post,
        Method::Put,
    ] {
        let (pseudos, _) = roundtrip(
            PseudoHeaders {
                method: Some(method),
                ..Default::default()
            },
            &headers,
        );
        assert_eq!(pseudos.method, Some(method), "method {method}");
    }
}

#[test]
fn roundtrip_non_standard_method() {
    let (pseudos, _) = roundtrip(
        PseudoHeaders {
            method: Some(Method::Patch),
            ..Default::default()
        },
        &Headers::new(),
    );
    assert_eq!(pseudos.method, Some(Method::Patch));
}

#[test]
fn roundtrip_all_static_table_statuses() {
    let headers = Headers::new();
    for status in [
        Status::Continue,
        Status::EarlyHints,
        Status::Ok,
        Status::NoContent,
        Status::PartialContent,
        Status::Found,
        Status::NotModified,
        Status::BadRequest,
        Status::Forbidden,
        Status::NotFound,
        Status::MisdirectedRequest,
        Status::TooEarly,
        Status::InternalServerError,
        Status::ServiceUnavailable,
    ] {
        let (pseudos, _) = roundtrip(
            PseudoHeaders {
                status: Some(status),
                ..Default::default()
            },
            &headers,
        );
        assert_eq!(pseudos.status, Some(status), "status {status}");
    }
}

#[test]
fn roundtrip_non_standard_status() {
    let (pseudos, _) = roundtrip(
        PseudoHeaders {
            status: Some(Status::ImATeapot),
            ..Default::default()
        },
        &Headers::new(),
    );
    assert_eq!(pseudos.status, Some(Status::ImATeapot));
}

#[test]
fn roundtrip_empty_header_value() {
    let mut headers = Headers::new();
    headers.insert(KnownHeaderName::Server, "");
    let (_, decoded) = roundtrip(PseudoHeaders::default(), &headers);
    assert_eq!(decoded.get_str(KnownHeaderName::Server), Some(""));
}

#[test]
fn roundtrip_long_header_value() {
    let long_value = "x".repeat(300);
    let mut headers = Headers::new();
    headers.insert("x-long", HeaderValue::from(long_value.clone()));
    let (_, decoded) = roundtrip(PseudoHeaders::default(), &headers);
    assert_eq!(decoded.get_str("x-long"), Some(long_value.as_str()));
}

// --- Decoder error path tests ---

#[test]
fn decode_empty_input() {
    assert!(FieldSection::decode(&[]).is_err());
}

#[test]
fn decode_truncated_prefix() {
    assert!(FieldSection::decode(&[0x00]).is_err());
}

#[test]
fn decode_nonzero_required_insert_count() {
    assert!(FieldSection::decode(&[0x01, 0x00]).is_err());
}

#[test]
fn decode_dynamic_table_indexed() {
    assert!(FieldSection::decode(&[0x00, 0x00, 0x10]).is_err());
}

#[test]
fn decode_dynamic_table_name_ref() {
    assert!(FieldSection::decode(&[0x00, 0x00, 0x00]).is_err());
}

#[test]
fn decode_static_index_out_of_range() {
    let buf = vec![0x00, 0x00, 0xFF, 0x24];
    assert!(FieldSection::decode(&buf).is_err());
}

#[test]
fn decode_truncated_string_value() {
    let buf = vec![
        0x00, 0x00, // prefix
        0x51, // literal with name ref, static, index 1
        0x05, // string length 5, no huffman
        0x2f, 0x61, // only 2 bytes of the promised 5
    ];
    assert!(FieldSection::decode(&buf).is_err());
}
