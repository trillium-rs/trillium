#![allow(dead_code)] // temporary
mod decoder;
mod encoder;
mod huffman;
mod static_table;
#[cfg(test)]
mod tests;
mod varint;

// §4.5.2: Indexed Field Line — first byte pattern 1xxxxxxx
const INDEXED_FIELD_LINE: u8 = 0x80;
const INDEXED_STATIC_FLAG: u8 = 0x40; // T bit

// §4.5.4: Literal Field Line with Name Reference — first byte pattern 01xxxxxx
const LITERAL_WITH_NAME_REF: u8 = 0x40;
const NAME_REF_STATIC_FLAG: u8 = 0x10; // T bit

// §4.5.6: Literal Field Line with Literal Name — first byte pattern 001xxxxx
const LITERAL_WITH_LITERAL_NAME: u8 = 0x20;

use crate::{Method, Status};
pub(crate) use decoder::decode_field_section;
pub(crate) use encoder::encode_field_section;
use std::borrow::Cow;

/// The six defined HTTP/3 pseudo-header fields (RFC 9114 §4.3, RFC 9220).
///
/// Unlike regular headers, pseudo-headers are a fixed set — unknown
/// pseudo-headers are a protocol error. Each may appear at most once.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PseudoHeaders<'a> {
    pub(crate) method: Option<Method>,
    pub(crate) status: Option<Status>,
    pub(crate) path: Option<Cow<'a, str>>,
    pub(crate) scheme: Option<Cow<'a, str>>,
    pub(crate) authority: Option<Cow<'a, str>>,
    pub(crate) protocol: Option<Cow<'a, str>>,
}
