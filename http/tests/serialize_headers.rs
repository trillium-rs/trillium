#![cfg(feature = "serde")]

#[test]
fn header_serialization() {
    let mut headers = trillium_http::Headers::new();
    headers.insert(trillium_http::KnownHeaderName::Accept, "Known");
    headers.insert(
        "non-utf8",
        vec![
            0xC0u8, 0xC1, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xFD, 0xFE, 0xFF,
        ],
    );
    headers.insert("multi-values", "value1");
    headers.append("multi-values", "value2");
    headers.append("multi-values", "value3");
    assert_eq!(
        serde_json::json!({
            "Accept": "Known",
            "non-utf8": vec![
                0xC0u8, 0xC1, 0xF5, 0xF6, 0xF7, 0xF8, 0xF9, 0xFA, 0xFB, 0xFC, 0xFD, 0xFE, 0xFF,
            ],
            "multi-values": ["value1", "value2", "value3"]
        }),
        serde_json::to_value(&headers).unwrap()
    );
}
