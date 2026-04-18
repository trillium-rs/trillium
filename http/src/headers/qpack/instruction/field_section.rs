//! Typed parser and wire-format encoders for QPACK field-section bytes (RFC 9204 §4.5).
//!
//! Field sections are the payload of HTTP/3 `HEADERS` and `PUSH_PROMISE` frames. Unlike the
//! encoder-stream (§3.2) and decoder-stream (§4.4) wire vocabularies, a field section arrives
//! as a complete byte slice, so [`FieldLineInstruction::parse`] is synchronous and borrows
//! from the input.
//!
//! [`FieldLineInstruction`] is the pure wire representation — each variant carries raw
//! indices (static / relative / post-base) and a [`FieldLineValue`] for literal values.
//! Dynamic-table resolution (turning a relative index into an entry name+value) is the
//! consumer's job; the parser has no table reference.
//!
//! [`FieldSectionPrefix`] carries the §4.5.1 prefix in its encoded form. Converting the
//! encoded Required Insert Count to an actual insert count requires table state (`max_entries`
//! and `total_inserts`), so that step lives at the semantic layer.

use crate::headers::qpack::{
    FieldLineValue, QpackEntryName, QpackError, huffman, instruction::encode_string, varint,
};

// --- §4.5.1 Field Section Prefix ---

/// Sign bit of the delta-base (§4.5.1.2). Set when `base < required_insert_count`.
const BASE_DELTA_SIGN: u8 = 0x80;

// --- §4.5.2 Indexed Field Line: 1Txxxxxx ---

const INDEXED_FIELD_LINE: u8 = 0x80;
/// T bit inside §4.5.2 (static when set, dynamic when clear).
const INDEXED_STATIC_FLAG: u8 = 0x40;

// --- §4.5.3 Indexed Field Line with Post-Base Index: 0001xxxx ---

const POST_BASE_INDEXED: u8 = 0x10;

// --- §4.5.4 Literal Field Line with Name Reference: 01NTxxxx ---

const LITERAL_WITH_NAME_REF: u8 = 0x40;
/// N (Never-Indexed) bit inside §4.5.4.
const NAME_REF_NEVER_INDEXED_FLAG: u8 = 0x20;
/// T bit inside §4.5.4 (static when set, dynamic when clear).
const NAME_REF_STATIC_FLAG: u8 = 0x10;

// --- §4.5.5 Literal Field Line with Post-Base Name Reference: 0000Nxxx ---

/// N (Never-Indexed) bit inside §4.5.5.
const POST_BASE_NAME_REF_NEVER_INDEXED_FLAG: u8 = 0x08;

// --- §4.5.6 Literal Field Line with Literal Name: 001NHxxx ---

const LITERAL_WITH_LITERAL_NAME: u8 = 0x20;
/// N (Never-Indexed) bit inside §4.5.6.
const LITERAL_NAME_NEVER_INDEXED_FLAG: u8 = 0x10;

/// One field line in its wire-encoded form (RFC 9204 §4.5.2–§4.5.6).
///
/// Indexed variants carry only the raw wire index. Literal variants carry the literal value
/// (and literal name, in the §4.5.6 case) as a [`FieldLineValue`] borrowing from the input
/// bytes on the non-Huffman path. The `never_indexed` flag on the four literal variants
/// carries the N bit per spec — preserved across decode and encode so intermediaries
/// (trillium-proxy) can re-emit sensitive headers without inserting them into a shared
/// dynamic table.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::headers) enum FieldLineInstruction<'a> {
    /// §4.5.2 (T=1): reference to the static table.
    IndexedStatic { index: usize },
    /// §4.5.2 (T=0): reference to a dynamic-table entry with absolute index
    /// `base - 1 - relative_index`.
    IndexedDynamic { relative_index: usize },
    /// §4.5.3: reference to a dynamic-table entry with absolute index
    /// `base + post_base_index`.
    IndexedPostBase { post_base_index: usize },
    /// §4.5.4 (T=1): literal value with a static-table name reference.
    LiteralStaticNameRef {
        name_index: usize,
        value: FieldLineValue<'a>,
        never_indexed: bool,
    },
    /// §4.5.4 (T=0): literal value with a dynamic-table pre-base name reference.
    LiteralDynamicNameRef {
        relative_index: usize,
        value: FieldLineValue<'a>,
        never_indexed: bool,
    },
    /// §4.5.5: literal value with a dynamic-table post-base name reference.
    LiteralPostBaseNameRef {
        post_base_index: usize,
        value: FieldLineValue<'a>,
        never_indexed: bool,
    },
    /// §4.5.6: literal name and literal value, no table reference.
    LiteralLiteralName {
        name: QpackEntryName<'a>,
        value: FieldLineValue<'a>,
        never_indexed: bool,
    },
}

impl<'a> FieldLineInstruction<'a> {
    /// Parse a single field line from the start of `input`. Returns the instruction and the
    /// unconsumed tail.
    pub(in crate::headers) fn parse(input: &'a [u8]) -> Result<(Self, &'a [u8]), QpackError> {
        let &[first, ..] = input else {
            return Err(QpackError::UnexpectedEnd);
        };

        if first & INDEXED_FIELD_LINE != 0 {
            // §4.5.2: 1Txxxxxx
            let (index, rest) = varint::decode(input, 6)?;
            let instr = if first & INDEXED_STATIC_FLAG != 0 {
                FieldLineInstruction::IndexedStatic { index }
            } else {
                FieldLineInstruction::IndexedDynamic {
                    relative_index: index,
                }
            };
            Ok((instr, rest))
        } else if first & LITERAL_WITH_NAME_REF != 0 {
            // §4.5.4: 01NTxxxx
            let never_indexed = first & NAME_REF_NEVER_INDEXED_FLAG != 0;
            let is_static = first & NAME_REF_STATIC_FLAG != 0;
            let (index, rest) = varint::decode(input, 4)?;
            let (value, rest) = decode_string(rest, 7)?;
            let instr = if is_static {
                FieldLineInstruction::LiteralStaticNameRef {
                    name_index: index,
                    value,
                    never_indexed,
                }
            } else {
                FieldLineInstruction::LiteralDynamicNameRef {
                    relative_index: index,
                    value,
                    never_indexed,
                }
            };
            Ok((instr, rest))
        } else if first & LITERAL_WITH_LITERAL_NAME != 0 {
            // §4.5.6: 001NHxxx
            let never_indexed = first & LITERAL_NAME_NEVER_INDEXED_FLAG != 0;
            let (name, rest) = decode_name(input, 3)?;
            let (value, rest) = decode_string(rest, 7)?;
            Ok((
                FieldLineInstruction::LiteralLiteralName {
                    name,
                    value,
                    never_indexed,
                },
                rest,
            ))
        } else if first & POST_BASE_INDEXED != 0 {
            // §4.5.3: 0001xxxx
            let (post_base_index, rest) = varint::decode(input, 4)?;
            Ok((
                FieldLineInstruction::IndexedPostBase { post_base_index },
                rest,
            ))
        } else {
            // §4.5.5: 0000Nxxx
            let never_indexed = first & POST_BASE_NAME_REF_NEVER_INDEXED_FLAG != 0;
            let (post_base_index, rest) = varint::decode(input, 3)?;
            let (value, rest) = decode_string(rest, 7)?;
            Ok((
                FieldLineInstruction::LiteralPostBaseNameRef {
                    post_base_index,
                    value,
                    never_indexed,
                },
                rest,
            ))
        }
    }

    /// Append the wire-format encoding of this instruction to `buf`.
    pub(in crate::headers) fn encode(&self, buf: &mut Vec<u8>) {
        match *self {
            FieldLineInstruction::IndexedStatic { index } => {
                let start = buf.len();
                buf.extend_from_slice(&varint::encode(index, 6));
                buf[start] |= INDEXED_FIELD_LINE | INDEXED_STATIC_FLAG;
            }
            FieldLineInstruction::IndexedDynamic { relative_index } => {
                let start = buf.len();
                buf.extend_from_slice(&varint::encode(relative_index, 6));
                buf[start] |= INDEXED_FIELD_LINE;
            }
            FieldLineInstruction::IndexedPostBase { post_base_index } => {
                let start = buf.len();
                buf.extend_from_slice(&varint::encode(post_base_index, 4));
                buf[start] |= POST_BASE_INDEXED;
            }
            FieldLineInstruction::LiteralStaticNameRef {
                name_index,
                ref value,
                never_indexed,
            } => {
                let start = buf.len();
                buf.extend_from_slice(&varint::encode(name_index, 4));
                buf[start] |= LITERAL_WITH_NAME_REF
                    | NAME_REF_STATIC_FLAG
                    | if never_indexed {
                        NAME_REF_NEVER_INDEXED_FLAG
                    } else {
                        0
                    };
                encode_string(value.as_bytes(), 7, buf);
            }
            FieldLineInstruction::LiteralDynamicNameRef {
                relative_index,
                ref value,
                never_indexed,
            } => {
                let start = buf.len();
                buf.extend_from_slice(&varint::encode(relative_index, 4));
                buf[start] |= LITERAL_WITH_NAME_REF
                    | if never_indexed {
                        NAME_REF_NEVER_INDEXED_FLAG
                    } else {
                        0
                    };
                encode_string(value.as_bytes(), 7, buf);
            }
            FieldLineInstruction::LiteralPostBaseNameRef {
                post_base_index,
                ref value,
                never_indexed,
            } => {
                let start = buf.len();
                buf.extend_from_slice(&varint::encode(post_base_index, 3));
                buf[start] |= if never_indexed {
                    POST_BASE_NAME_REF_NEVER_INDEXED_FLAG
                } else {
                    0
                };
                encode_string(value.as_bytes(), 7, buf);
            }
            FieldLineInstruction::LiteralLiteralName {
                ref name,
                ref value,
                never_indexed,
            } => {
                let start = buf.len();
                encode_string(name.as_bytes(), 3, buf);
                buf[start] |= LITERAL_WITH_LITERAL_NAME
                    | if never_indexed {
                        LITERAL_NAME_NEVER_INDEXED_FLAG
                    } else {
                        0
                    };
                encode_string(value.as_bytes(), 7, buf);
            }
        }
    }
}

/// The §4.5.1 Field Section Prefix in its encoded form.
///
/// `encoded_required_insert_count` is the on-wire value — converting it to an actual insert
/// count requires the table's `max_entries` and `total_inserts`, which is table-state
/// business that lives at the semantic layer.
#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub(in crate::headers) struct FieldSectionPrefix {
    /// §4.5.1.1 encoded Required Insert Count (8-bit prefix varint). Zero means the section
    /// references no dynamic-table entries.
    pub encoded_required_insert_count: usize,
    /// §4.5.1.2 S bit: set when `base < required_insert_count`.
    pub base_is_negative: bool,
    /// §4.5.1.2 Delta Base (7-bit prefix varint).
    pub delta_base: usize,
}

impl FieldSectionPrefix {
    pub(in crate::headers) fn parse(input: &[u8]) -> Result<(Self, &[u8]), QpackError> {
        let (encoded_required_insert_count, rest) = varint::decode(input, 8)?;
        let &[first, ..] = rest else {
            return Err(QpackError::UnexpectedEnd);
        };
        let base_is_negative = first & BASE_DELTA_SIGN != 0;
        let (delta_base, rest) = varint::decode(rest, 7)?;
        Ok((
            Self {
                encoded_required_insert_count,
                base_is_negative,
                delta_base,
            },
            rest,
        ))
    }

    pub(in crate::headers) fn encode(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&varint::encode(self.encoded_required_insert_count, 8));
        let start = buf.len();
        buf.extend_from_slice(&varint::encode(self.delta_base, 7));
        if self.base_is_negative {
            buf[start] |= BASE_DELTA_SIGN;
        }
    }
}

// --- sync wire-read helpers ---

/// Decode a §4.1.2 string literal: H flag at bit `prefix_size`, then length (in the low
/// `prefix_size` bits as a varint), then the body.
///
/// On the non-Huffman path the returned value is `FieldLineValue::Borrowed(&input[..])` —
/// zero allocation. On the Huffman path the decoded bytes are owned.
fn decode_string(input: &[u8], prefix_size: u8) -> Result<(FieldLineValue<'_>, &[u8]), QpackError> {
    let &[first, ..] = input else {
        return Err(QpackError::UnexpectedEnd);
    };
    let huffman_encoded = first & (1 << prefix_size) != 0;
    let (length, rest) = varint::decode(input, prefix_size)?;
    if rest.len() < length {
        return Err(QpackError::UnexpectedEnd);
    }
    let (body, rest) = rest.split_at(length);
    let value = if huffman_encoded {
        FieldLineValue::Owned(huffman::decode(body)?)
    } else {
        FieldLineValue::Borrowed(body)
    };
    Ok((value, rest))
}

/// Decode a §4.1.2 name string into a [`QpackEntryName`], preserving the borrow on the
/// non-Huffman path.
fn decode_name(input: &[u8], prefix_size: u8) -> Result<(QpackEntryName<'_>, &[u8]), QpackError> {
    let &[first, ..] = input else {
        return Err(QpackError::UnexpectedEnd);
    };
    let huffman_encoded = first & (1 << prefix_size) != 0;
    let (length, rest) = varint::decode(input, prefix_size)?;
    if rest.len() < length {
        return Err(QpackError::UnexpectedEnd);
    }
    let (body, rest) = rest.split_at(length);
    let name = if huffman_encoded {
        QpackEntryName::try_from(huffman::decode(body)?)
            .map_err(|_| QpackError::InvalidHeaderName)?
    } else {
        QpackEntryName::try_from(body).map_err(|_| QpackError::InvalidHeaderName)?
    };
    Ok((name, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fv(s: &'static [u8]) -> FieldLineValue<'static> {
        FieldLineValue::Static(s)
    }

    #[allow(clippy::needless_pass_by_value)]
    fn roundtrip(instr: FieldLineInstruction<'static>) {
        let mut buf = Vec::new();
        instr.encode(&mut buf);
        let (parsed, rest) = FieldLineInstruction::parse(&buf).expect("parse succeeds");
        assert!(rest.is_empty(), "instruction left trailing bytes");
        assert_eq!(parsed, instr);
    }

    #[test]
    fn roundtrip_indexed_static() {
        roundtrip(FieldLineInstruction::IndexedStatic { index: 0 });
        roundtrip(FieldLineInstruction::IndexedStatic { index: 25 });
        roundtrip(FieldLineInstruction::IndexedStatic { index: 98 });
        // Large index to force multi-byte varint
        roundtrip(FieldLineInstruction::IndexedStatic { index: 10_000 });
    }

    #[test]
    fn roundtrip_indexed_dynamic() {
        roundtrip(FieldLineInstruction::IndexedDynamic { relative_index: 0 });
        roundtrip(FieldLineInstruction::IndexedDynamic { relative_index: 62 });
        roundtrip(FieldLineInstruction::IndexedDynamic { relative_index: 63 });
        roundtrip(FieldLineInstruction::IndexedDynamic {
            relative_index: 5_000,
        });
    }

    #[test]
    fn roundtrip_indexed_post_base() {
        roundtrip(FieldLineInstruction::IndexedPostBase { post_base_index: 0 });
        roundtrip(FieldLineInstruction::IndexedPostBase {
            post_base_index: 14,
        });
        roundtrip(FieldLineInstruction::IndexedPostBase {
            post_base_index: 15,
        });
        roundtrip(FieldLineInstruction::IndexedPostBase {
            post_base_index: 2_000,
        });
    }

    #[test]
    fn roundtrip_literal_static_name_ref() {
        for never_indexed in [false, true] {
            roundtrip(FieldLineInstruction::LiteralStaticNameRef {
                name_index: 15, // :status
                value: fv(b"200"),
                never_indexed,
            });
            roundtrip(FieldLineInstruction::LiteralStaticNameRef {
                name_index: 0,
                value: fv(b""),
                never_indexed,
            });
            roundtrip(FieldLineInstruction::LiteralStaticNameRef {
                name_index: 500,
                value: fv(b"a value long enough to exercise multi-byte varint"),
                never_indexed,
            });
        }
    }

    #[test]
    fn roundtrip_literal_dynamic_name_ref() {
        for never_indexed in [false, true] {
            roundtrip(FieldLineInstruction::LiteralDynamicNameRef {
                relative_index: 0,
                value: fv(b"v"),
                never_indexed,
            });
            roundtrip(FieldLineInstruction::LiteralDynamicNameRef {
                relative_index: 14,
                value: fv(b"v"),
                never_indexed,
            });
            roundtrip(FieldLineInstruction::LiteralDynamicNameRef {
                relative_index: 1_000,
                value: fv(b"longer value that might benefit from huffman"),
                never_indexed,
            });
        }
    }

    #[test]
    fn roundtrip_literal_post_base_name_ref() {
        for never_indexed in [false, true] {
            roundtrip(FieldLineInstruction::LiteralPostBaseNameRef {
                post_base_index: 0,
                value: fv(b"v"),
                never_indexed,
            });
            roundtrip(FieldLineInstruction::LiteralPostBaseNameRef {
                post_base_index: 6,
                value: fv(b"v"),
                never_indexed,
            });
            roundtrip(FieldLineInstruction::LiteralPostBaseNameRef {
                post_base_index: 500,
                value: fv(b"longer value"),
                never_indexed,
            });
        }
    }

    #[test]
    fn roundtrip_literal_literal_name() {
        for never_indexed in [false, true] {
            roundtrip(FieldLineInstruction::LiteralLiteralName {
                name: QpackEntryName::try_from(b"x-custom".as_slice())
                    .unwrap()
                    .into_owned(),
                value: fv(b"value"),
                never_indexed,
            });
            roundtrip(FieldLineInstruction::LiteralLiteralName {
                name: QpackEntryName::try_from(b"content-type".as_slice())
                    .unwrap()
                    .into_owned(),
                value: fv(b"application/json"),
                never_indexed,
            });
        }
    }

    #[test]
    fn field_section_prefix_roundtrip() {
        for prefix in [
            FieldSectionPrefix {
                encoded_required_insert_count: 0,
                base_is_negative: false,
                delta_base: 0,
            },
            FieldSectionPrefix {
                encoded_required_insert_count: 5,
                base_is_negative: false,
                delta_base: 0,
            },
            FieldSectionPrefix {
                encoded_required_insert_count: 5,
                base_is_negative: true,
                delta_base: 3,
            },
            FieldSectionPrefix {
                encoded_required_insert_count: 1_000,
                base_is_negative: false,
                delta_base: 500,
            },
        ] {
            let mut buf = Vec::new();
            prefix.encode(&mut buf);
            let (parsed, rest) = FieldSectionPrefix::parse(&buf).unwrap();
            assert!(rest.is_empty());
            assert_eq!(parsed, prefix);
        }
    }

    #[test]
    fn parse_multiple_field_lines() {
        let mut buf = Vec::new();
        FieldLineInstruction::IndexedStatic { index: 25 }.encode(&mut buf);
        FieldLineInstruction::LiteralStaticNameRef {
            name_index: 15,
            value: fv(b"200"),
            never_indexed: false,
        }
        .encode(&mut buf);
        FieldLineInstruction::IndexedDynamic { relative_index: 0 }.encode(&mut buf);

        let (a, rest) = FieldLineInstruction::parse(&buf).unwrap();
        let (b, rest) = FieldLineInstruction::parse(rest).unwrap();
        let (c, rest) = FieldLineInstruction::parse(rest).unwrap();
        assert!(rest.is_empty());
        assert_eq!(a, FieldLineInstruction::IndexedStatic { index: 25 });
        assert_eq!(
            b,
            FieldLineInstruction::LiteralStaticNameRef {
                name_index: 15,
                value: fv(b"200"),
                never_indexed: false,
            }
        );
        assert_eq!(
            c,
            FieldLineInstruction::IndexedDynamic { relative_index: 0 }
        );
    }

    #[test]
    fn parse_empty_input_is_error() {
        assert!(matches!(
            FieldLineInstruction::parse(&[]),
            Err(QpackError::UnexpectedEnd)
        ));
    }

    #[test]
    fn parse_truncated_string_literal() {
        // LiteralLiteralName with name length 5 but only 2 bytes available after the header.
        let buf = [0x20 | 0x05, b'a', b'b'];
        assert!(matches!(
            FieldLineInstruction::parse(&buf),
            Err(QpackError::UnexpectedEnd)
        ));
    }

    mod spec_vectors {
        //! Wire-level parse tests against the worked examples in RFC 9204 Appendix B.
        //!
        //! These assert that our §4.5 parser produces the exact typed instructions the
        //! spec documents for a given byte sequence. They don't attempt to round-trip
        //! through our encoder — our encoder makes different (and legitimate) policy
        //! choices around Huffman selection, base selection (pre-base vs post-base), and
        //! Duplicate emission.

        use super::*;

        #[track_caller]
        fn parse_all(bytes: &[u8]) -> (FieldSectionPrefix, Vec<FieldLineInstruction<'_>>) {
            let (prefix, mut rest) = FieldSectionPrefix::parse(bytes).expect("prefix parses");
            let mut lines = Vec::new();
            while !rest.is_empty() {
                let (instr, tail) = FieldLineInstruction::parse(rest).expect("field line parses");
                lines.push(instr);
                rest = tail;
            }
            (prefix, lines)
        }

        #[test]
        fn b1_literal_with_static_name_ref() {
            // `0000 51 0b 2f69 6e64 6578 2e68 746d 6c`
            // Prefix: RIC=0, Base=0. Field line: Literal With Name Reference, T=1,
            // name_index=1 (`:path`), literal value "/index.html".
            let bytes = [
                0x00, 0x00, 0x51, 0x0b, 0x2f, 0x69, 0x6e, 0x64, 0x65, 0x78, 0x2e, 0x68, 0x74, 0x6d,
                0x6c,
            ];
            let (prefix, lines) = parse_all(&bytes);
            assert_eq!(prefix, FieldSectionPrefix::default());
            assert_eq!(
                lines,
                vec![FieldLineInstruction::LiteralStaticNameRef {
                    name_index: 1,
                    value: FieldLineValue::Static(b"/index.html"),
                    never_indexed: false,
                }],
            );
        }

        #[test]
        fn b2_post_base_indexed_with_negative_base() {
            // `0381 10 11`
            // Prefix: encoded_ric=3, S=1, delta_base=1 → RIC=2, Base=0.
            // Two post-base indexed entries: post_base_index=0 and post_base_index=1.
            let bytes = [0x03, 0x81, 0x10, 0x11];
            let (prefix, lines) = parse_all(&bytes);
            assert_eq!(
                prefix,
                FieldSectionPrefix {
                    encoded_required_insert_count: 3,
                    base_is_negative: true,
                    delta_base: 1,
                },
            );
            assert_eq!(
                lines,
                vec![
                    FieldLineInstruction::IndexedPostBase { post_base_index: 0 },
                    FieldLineInstruction::IndexedPostBase { post_base_index: 1 },
                ],
            );
        }

        #[test]
        fn b4_mixed_dynamic_and_static_indexed() {
            // `0500 80 c1 81`
            // Prefix: encoded_ric=5, S=0, delta_base=0 → RIC=4, Base=4.
            // IndexedDynamic{0} → abs 3, IndexedStatic{1} → `:path /`, IndexedDynamic{1} → abs 2.
            let bytes = [0x05, 0x00, 0x80, 0xc1, 0x81];
            let (prefix, lines) = parse_all(&bytes);
            assert_eq!(
                prefix,
                FieldSectionPrefix {
                    encoded_required_insert_count: 5,
                    base_is_negative: false,
                    delta_base: 0,
                },
            );
            assert_eq!(
                lines,
                vec![
                    FieldLineInstruction::IndexedDynamic { relative_index: 0 },
                    FieldLineInstruction::IndexedStatic { index: 1 },
                    FieldLineInstruction::IndexedDynamic { relative_index: 1 },
                ],
            );
        }
    }
}
