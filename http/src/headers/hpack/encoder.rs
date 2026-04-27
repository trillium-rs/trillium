//! HPACK encoder (RFC 7541 §6), static-or-literal only.
//!
//! No dynamic-table insertion: every emitted field line is either §6.1 Indexed (when the
//! pair is a full match against the static table) or §6.2.2 Literal without Indexing. This
//! matches QPACK's current encoder shape and avoids the compression-theory morass of
//! dynamic insert selection — the space has no satisfying standard solution and it's an
//! incremental improvement once the static baseline is stable.
//!
//! Strings use Huffman (§5.2) when strictly shorter than the raw form; otherwise plain.
//!
//! No size updates: this encoder does not resize the decoder's dynamic table. The peer's
//! `SETTINGS_HEADER_TABLE_SIZE` announcement governs the ceiling on the decoder side; we
//! simply never put anything there.

#[cfg(test)]
mod tests;

use super::static_table::{StaticLookup, static_table_lookup};
use crate::headers::{field_section::FieldSection, huffman, integer_prefix};

/// Encode `field_section` into `buf` as an HPACK header block.
pub fn encode(field_section: &FieldSection<'_>, buf: &mut Vec<u8>) {
    for (name, value) in field_section.field_lines() {
        let value_bytes = value.as_bytes();
        match static_table_lookup(&name, value_bytes) {
            StaticLookup::FullMatch(idx) => encode_indexed(idx, buf),
            StaticLookup::NameMatch(idx) => {
                encode_literal_without_indexing_name_ref(idx, value_bytes, buf);
            }
            StaticLookup::NoMatch => {
                encode_literal_without_indexing_literal_name(name.as_bytes(), value_bytes, buf);
            }
        }
    }
}

/// §6.1 Indexed Header Field: `1xxxxxxx` + 7-bit prefix integer.
fn encode_indexed(index: u8, buf: &mut Vec<u8>) {
    let start = buf.len();
    integer_prefix::encode_into(index as usize, 7, buf);
    buf[start] |= 0b1000_0000;
}

/// §6.2.2 Literal Header Field without Indexing, static name reference: `0000xxxx` + 4-bit
/// prefix index + value string.
fn encode_literal_without_indexing_name_ref(index: u8, value: &[u8], buf: &mut Vec<u8>) {
    // Top 4 bits are zero — no flag to OR in. `integer_prefix::encode_into` with a 4-bit
    // prefix leaves those bits cleared, which is exactly the §6.2.2 representation.
    integer_prefix::encode_into(index as usize, 4, buf);
    encode_string(value, buf);
}

/// §6.2.2 Literal Header Field without Indexing, literal name: `00000000` + name string +
/// value string.
fn encode_literal_without_indexing_literal_name(name: &[u8], value: &[u8], buf: &mut Vec<u8>) {
    buf.push(0);
    encode_string(name, buf);
    encode_string(value, buf);
}

/// §5.2 string literal: H flag + 7-bit length prefix + bytes. Huffman-encodes when strictly
/// shorter.
fn encode_string(s: &[u8], buf: &mut Vec<u8>) {
    let start = buf.len();
    if let Some(huffman_len) = huffman::encoded_length_if_shorter(s) {
        integer_prefix::encode_into(huffman_len, 7, buf);
        buf[start] |= 0b1000_0000;
        huffman::encode_into(s, buf);
    } else {
        integer_prefix::encode_into(s.len(), 7, buf);
        buf.extend_from_slice(s);
    }
}
