//! Typed parser and wire-format encoders for QPACK encoder-stream instructions
//! (RFC 9204 §3.2).
//!
//! [`parse`] reads one instruction off the wire and returns it as an [`EncoderInstruction`]
//! without applying it to any table. The consumer ([`decoder_dynamic_table::DecoderDynamicTable`],
//! and encoder-side tests) dispatches the parsed value.
//!
//! Name resolution for dynamic-name-ref inserts is *deferred* to the dispatcher — the parser
//! has no reference to the peer's dynamic table. Huffman decoding of names and values is
//! performed at parse time so the enum carries plain byte buffers.
//!
//! The `encode_*` functions are the symmetric wire encoders. They are used by
//! [`encoder_dynamic_table::EncoderDynamicTable`] to enqueue insert and capacity instructions
//! for the encoder-stream writer; this is the only place the raw bit patterns are needed.
//!
//! [`decoder_dynamic_table::DecoderDynamicTable`]: crate::headers::qpack::decoder_dynamic_table::DecoderDynamicTable
//! [`encoder_dynamic_table::EncoderDynamicTable`]: crate::headers::qpack::encoder_dynamic_table::EncoderDynamicTable

use super::{
    encode_string, read_exact, read_first_byte, read_string_with_huffman, read_varint,
    validate_value,
};
use crate::{
    h3::{H3Error, H3ErrorCode},
    headers::qpack::{entry_name::QpackEntryName, huffman, varint},
};
use futures_lite::io::AsyncRead;

// §3.2.2: Insert With Name Reference — first byte pattern 1xxxxxxx.
const INSERT_WITH_NAME_REF: u8 = 0x80;
// T bit within Insert With Name Reference — set for static-table references.
const NAME_REF_STATIC_FLAG: u8 = 0x40;

// §3.2.3: Insert With Literal Name — first byte pattern 01xxxxxx.
const INSERT_WITH_LITERAL_NAME: u8 = 0x40;
// H bit for the name string of Insert With Literal Name.
const LITERAL_NAME_HUFFMAN_FLAG: u8 = 0x20;

// §3.2.1: Set Dynamic Table Capacity — first byte pattern 001xxxxx.
const SET_DYNAMIC_TABLE_CAPACITY: u8 = 0x20;

// §3.2.4: Duplicate — first byte pattern 000xxxxx. The high bits are already zero, so the
// constant is just documentation for the encode path (no OR-in needed).
const DUPLICATE: u8 = 0x00;

/// One parsed encoder-stream instruction (RFC 9204 §3.2).
///
/// Dynamic-name-ref variants carry the relative index rather than the resolved name: the
/// parser has no dynamic-table reference and callers resolve indices at apply time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::headers) enum EncoderInstruction {
    /// §3.2.1: Set Dynamic Table Capacity.
    SetCapacity(usize),
    /// §3.2.2: Insert With Name Reference (T=1, static table).
    InsertWithStaticNameRef { name_index: usize, value: Vec<u8> },
    /// §3.2.2: Insert With Name Reference (T=0, dynamic table). `relative_index` is as seen
    /// on the wire; the dispatcher resolves it against the current dynamic-table contents.
    InsertWithDynamicNameRef {
        relative_index: usize,
        value: Vec<u8>,
    },
    /// §3.2.3: Insert With Literal Name.
    InsertWithLiteralName {
        name: QpackEntryName<'static>,
        value: Vec<u8>,
    },
    /// §3.2.4: Duplicate.
    Duplicate { relative_index: usize },
}

/// Parse the next encoder-stream instruction from `stream`.
///
/// `max_entry_size` is our advertised `SETTINGS_QPACK_MAX_TABLE_CAPACITY` and bounds each
/// individual name/value string length before allocation (RFC 9204 §3.2.2). Entries larger
/// than our advertised capacity are invalid regardless, so rejecting at read time prevents
/// a peer from forcing a huge allocation via a single length prefix.
///
/// Returns `Ok(None)` on clean EOF between instructions. `Ok(Some(_))` is a parsed
/// instruction; `Err` is an I/O or wire-format error mapped to `QpackEncoderStreamError`.
pub(in crate::headers) async fn parse(
    max_entry_size: usize,
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Option<EncoderInstruction>, H3Error> {
    parse_inner(max_entry_size, stream)
        .await
        .map_err(|()| H3ErrorCode::QpackEncoderStreamError.into())
}

async fn parse_inner(
    max_entry_size: usize,
    stream: &mut (impl AsyncRead + Unpin),
) -> Result<Option<EncoderInstruction>, ()> {
    let Some(first) = read_first_byte(stream).await? else {
        return Ok(None);
    };

    let instr = if first & INSERT_WITH_NAME_REF != 0 {
        // §3.2.2: Insert With Name Reference — 1Txxxxxx
        let is_static = first & NAME_REF_STATIC_FLAG != 0;
        let index = read_varint(first, 6, stream).await?;
        let value = read_string_with_huffman(max_entry_size, stream).await?;
        validate_value(&value)?;
        if is_static {
            EncoderInstruction::InsertWithStaticNameRef {
                name_index: index,
                value,
            }
        } else {
            EncoderInstruction::InsertWithDynamicNameRef {
                relative_index: index,
                value,
            }
        }
    } else if first & INSERT_WITH_LITERAL_NAME != 0 {
        // §3.2.3: Insert With Literal Name — 01HXXXXX
        let is_huffman = first & LITERAL_NAME_HUFFMAN_FLAG != 0;
        let name_len = read_varint(first, 5, stream).await?;
        let name_bytes = read_exact(name_len, max_entry_size, stream).await?;
        let name_bytes = if is_huffman {
            huffman::decode(&name_bytes).map_err(|e| {
                log::error!("QPACK encoder: huffman name decode failed: {e:?}");
            })?
        } else {
            name_bytes
        };
        let name = QpackEntryName::try_from(name_bytes).map_err(|e| {
            log::error!("QPACK encoder: invalid literal name: {e:?}");
        })?;
        let value = read_string_with_huffman(max_entry_size, stream).await?;
        validate_value(&value)?;
        EncoderInstruction::InsertWithLiteralName { name, value }
    } else if first & SET_DYNAMIC_TABLE_CAPACITY != 0 {
        // §3.2.1: Set Dynamic Table Capacity — 001XXXXX
        let capacity = read_varint(first, 5, stream).await?;
        EncoderInstruction::SetCapacity(capacity)
    } else {
        // §3.2.4: Duplicate — 000XXXXX
        let relative_index = read_varint(first, 5, stream).await?;
        EncoderInstruction::Duplicate { relative_index }
    };

    Ok(Some(instr))
}

// --- §3.2 wire encoders ---

/// Set Dynamic Table Capacity (§3.2.1): `001xxxxx` with a 5-bit prefix integer.
pub(in crate::headers) fn encode_set_capacity(capacity: usize) -> Vec<u8> {
    let mut bytes = varint::encode(capacity, 5);
    bytes[0] |= SET_DYNAMIC_TABLE_CAPACITY;
    bytes
}

/// Insert With Literal Name (§3.2.3): `01HNNNNN` with a 5-bit name-length prefix, followed
/// by the name bytes, then a string literal for the value.
pub(in crate::headers) fn encode_insert_with_literal_name(name: &[u8], value: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(name.len() + value.len() + 4);
    let name_start = buf.len();
    encode_string(name, 5, &mut buf);
    buf[name_start] |= INSERT_WITH_LITERAL_NAME;
    encode_string(value, 7, &mut buf);
    buf
}

/// Insert With Name Reference (§3.2.2): `1THNNNNN...` — 6-bit prefix integer for the name
/// index (T selects static vs dynamic), followed by a string literal for the value.
pub(in crate::headers) fn encode_insert_with_name_ref(
    name_index: usize,
    is_static: bool,
    value: &[u8],
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(value.len() + 4);
    let start = buf.len();
    buf.extend_from_slice(&varint::encode(name_index, 6));
    buf[start] |= INSERT_WITH_NAME_REF | if is_static { NAME_REF_STATIC_FLAG } else { 0 };
    encode_string(value, 7, &mut buf);
    buf
}

/// Duplicate (§3.2.4): `000xxxxx` — 5-bit prefix integer for the relative index.
pub(in crate::headers) fn encode_duplicate(relative_index: usize) -> Vec<u8> {
    let mut bytes = varint::encode(relative_index, 5);
    bytes[0] |= DUPLICATE;
    bytes
}

#[cfg(test)]
mod spec_vectors {
    //! Wire-level parse tests against the worked examples in RFC 9204 Appendix B.
    //!
    //! These assert that our §3.2 parser produces the exact interpretation the spec
    //! documents for a given byte sequence. They don't attempt to round-trip through our
    //! encoder — our encoder makes different (and legitimate) policy choices around
    //! Huffman selection, Duplicate emission, and base selection (see
    //! `qpack-field-line-instruction-plan` memory).

    use super::*;
    use futures_lite::future::block_on;

    #[track_caller]
    fn parse_one(mut bytes: &[u8]) -> EncoderInstruction {
        let instr = block_on(parse(usize::MAX, &mut bytes))
            .expect("parse ok")
            .expect("not eof");
        assert!(bytes.is_empty(), "trailing bytes after parse: {bytes:?}");
        instr
    }

    #[test]
    fn b2_set_capacity_220() {
        // `3fbd01` — Set Dynamic Table Capacity with 5-bit-prefix varint = 220.
        assert_eq!(
            parse_one(&[0x3f, 0xbd, 0x01]),
            EncoderInstruction::SetCapacity(220),
        );
    }

    #[test]
    fn b2_insert_name_ref_static_authority() {
        // `c0 0f 77 77 77 2e 65 78 61 6d 70 6c 65 2e 63 6f 6d`
        // Insert With Name Reference, T=1, name_index=0 (`:authority`), literal value.
        let bytes = [
            0xc0, 0x0f, 0x77, 0x77, 0x77, 0x2e, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x2e,
            0x63, 0x6f, 0x6d,
        ];
        assert_eq!(
            parse_one(&bytes),
            EncoderInstruction::InsertWithStaticNameRef {
                name_index: 0,
                value: b"www.example.com".to_vec(),
            },
        );
    }

    #[test]
    fn b2_insert_name_ref_static_path() {
        // `c1 0c 2f 73 61 6d 70 6c 65 2f 70 61 74 68`
        let bytes = [
            0xc1, 0x0c, 0x2f, 0x73, 0x61, 0x6d, 0x70, 0x6c, 0x65, 0x2f, 0x70, 0x61, 0x74, 0x68,
        ];
        assert_eq!(
            parse_one(&bytes),
            EncoderInstruction::InsertWithStaticNameRef {
                name_index: 1,
                value: b"/sample/path".to_vec(),
            },
        );
    }

    #[test]
    fn b3_insert_literal_name_custom_key() {
        // `4a 63 75 73 74 6f 6d 2d 6b 65 79 0c 63 75 73 74 6f 6d 2d 76 61 6c 75 65`
        // Insert With Literal Name, H=0, name_len=10 "custom-key", value "custom-value".
        let bytes = [
            0x4a, 0x63, 0x75, 0x73, 0x74, 0x6f, 0x6d, 0x2d, 0x6b, 0x65, 0x79, 0x0c, 0x63, 0x75,
            0x73, 0x74, 0x6f, 0x6d, 0x2d, 0x76, 0x61, 0x6c, 0x75, 0x65,
        ];
        assert_eq!(
            parse_one(&bytes),
            EncoderInstruction::InsertWithLiteralName {
                name: QpackEntryName::try_from(b"custom-key".to_vec()).unwrap(),
                value: b"custom-value".to_vec(),
            },
        );
    }

    #[test]
    fn b4_duplicate_relative_index_2() {
        assert_eq!(
            parse_one(&[0x02]),
            EncoderInstruction::Duplicate { relative_index: 2 },
        );
    }

    #[test]
    fn b5_insert_name_ref_dynamic() {
        // `81 0d 63 75 73 74 6f 6d 2d 76 61 6c 75 65 32`
        // Insert With Name Reference, T=0, relative_index=1, value "custom-value2".
        let bytes = [
            0x81, 0x0d, 0x63, 0x75, 0x73, 0x74, 0x6f, 0x6d, 0x2d, 0x76, 0x61, 0x6c, 0x75, 0x65,
            0x32,
        ];
        assert_eq!(
            parse_one(&bytes),
            EncoderInstruction::InsertWithDynamicNameRef {
                relative_index: 1,
                value: b"custom-value2".to_vec(),
            },
        );
    }
}
