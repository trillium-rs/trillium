//! QPACK types
//!
//! Please note that this interface is likely to change

#[cfg(test)]
mod corpus_tests;
mod decoder;
mod decoder_dynamic_table;
mod encoder;
mod encoder_dynamic_table;
pub(crate) mod huffman;
pub(crate) mod static_table;
#[cfg(test)]
mod tests;
pub(crate) mod varint;

#[cfg(not(feature = "unstable"))]
pub(crate) use decoder_dynamic_table::DecoderDynamicTable;
#[cfg(feature = "unstable")]
pub use decoder_dynamic_table::DecoderDynamicTable;
#[cfg(not(feature = "unstable"))]
pub(crate) use encoder_dynamic_table::EncoderDynamicTable;
#[cfg(feature = "unstable")]
pub use encoder_dynamic_table::EncoderDynamicTable;
#[cfg(feature = "unstable")]
pub use huffman::HuffmanError;

// --- Field section representation (RFC 9204 §4.5) ---

// §4.5.1: Field Section Prefix — sign bit of delta base
pub(crate) const BASE_DELTA_SIGN: u8 = 0x80;

// §4.5.2: Indexed Field Line — first byte pattern 1xxxxxxx
pub(crate) const INDEXED_FIELD_LINE: u8 = 0x80;
pub(crate) const INDEXED_STATIC_FLAG: u8 = 0x40; // T bit

// §4.5.3: Indexed Field Line with Post-Base Index — first byte pattern 0001xxxx
pub(crate) const POST_BASE_INDEXED: u8 = 0x10;

// §4.5.4: Literal Field Line with Name Reference — first byte pattern 01xxxxxx
pub(crate) const LITERAL_WITH_NAME_REF: u8 = 0x40;
pub(crate) const NAME_REF_STATIC_FLAG: u8 = 0x10; // T bit

// §4.5.5: Literal Field Line with Post-Base Name Reference — first byte pattern 0000xxxx
// (no constant needed; it is the else case after all above patterns are checked)

// §4.5.6: Literal Field Line with Literal Name — first byte pattern 001xxxxx
pub(crate) const LITERAL_WITH_LITERAL_NAME: u8 = 0x20;

// --- Encoder stream instructions (RFC 9204 §3.2) ---

// §3.2.2: Insert With Name Reference — first byte pattern 1xxxxxxx
pub(crate) const ENC_INSTR_INSERT_WITH_NAME_REF: u8 = 0x80;
pub(crate) const ENC_INSTR_NAME_REF_STATIC_FLAG: u8 = 0x40; // T bit

// §3.2.3: Insert With Literal Name — first byte pattern 01xxxxxx
pub(crate) const ENC_INSTR_INSERT_WITH_LITERAL_NAME: u8 = 0x40;
pub(crate) const ENC_INSTR_LITERAL_NAME_HUFFMAN_FLAG: u8 = 0x20; // H bit for name

// §3.2.1: Set Dynamic Table Capacity — first byte pattern 001xxxxx
pub(crate) const ENC_INSTR_SET_DYNAMIC_TABLE_CAPACITY: u8 = 0x20;

// §3.2.4: Duplicate — first byte pattern 000xxxxx
// (no constant needed; it is the else case after all above patterns are checked)

// --- Decoder stream instructions (RFC 9204 §4.4) ---

// §4.4.1: Section Acknowledgement — first byte pattern 1xxxxxxx with 7-bit stream ID
pub(crate) const DEC_INSTR_SECTION_ACK: u8 = 0x80;

// §4.4.2: Stream Cancellation — first byte pattern 01xxxxxx (not yet implemented)

// §4.4.3: Insert Count Increment — first byte pattern 00xxxxxx with 6-bit increment
// High bits are 0x00, so no OR-in needed when encoding; constant serves as documentation.
pub(crate) const DEC_INSTR_INSERT_COUNT_INC: u8 = 0x00;

// --- String literals (RFC 9204 §4.1.2) ---

// H flag (Huffman) in a string literal with a 7-bit length prefix (e.g. value strings
// in encoder instructions and field section value fields).
pub(crate) const STRING_HUFFMAN_FLAG: u8 = 0x80;

use crate::{Headers, Method, Status};
use fieldwork::Fieldwork;
use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
};

/// The six defined HTTP/3 pseudo-header fields (RFC 9114 §4.3, RFC 9220).
///
/// Unlike regular headers, pseudo-headers are a fixed set — unknown
/// pseudo-headers are a protocol error. Each may appear at most once.
#[derive(Debug, Default, Clone, PartialEq, Eq, Fieldwork)]
#[fieldwork(get, take, with, without, set, into)]
pub struct PseudoHeaders<'a> {
    /// :method pseudo header
    #[field(copy)]
    method: Option<Method>,

    /// :status pseudo header
    #[field(copy)]
    status: Option<Status>,

    /// :path pseudo header
    path: Option<Cow<'a, str>>,

    /// :scheme pseudo header
    scheme: Option<Cow<'a, str>>,

    /// :authority pseudo header
    authority: Option<Cow<'a, str>>,

    /// :protocol pseudo header
    protocol: Option<Cow<'a, str>>,
}

impl Display for PseudoHeaders<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(method) = &self.method {
            writeln!(f, ":method: {method}")?;
        }

        if let Some(status) = &self.status {
            writeln!(f, ":status: {status}")?;
        }

        if let Some(path) = &self.path {
            writeln!(f, ":path: {path}")?;
        }
        if let Some(scheme) = &self.scheme {
            writeln!(f, ":scheme: {scheme}")?;
        }

        if let Some(authority) = &self.authority {
            writeln!(f, ":authority: {authority}")?;
        }

        if let Some(protocol) = &self.protocol {
            writeln!(f, ":protocol: {protocol}")?;
        }

        Ok(())
    }
}

/// Combined [`PseudoHeaders`] and [`Headers`]
#[derive(Debug, Clone, Fieldwork)]
#[fieldwork(get, get_mut, into_field)]
pub struct FieldSection<'a> {
    /// pseudo-headers
    pseudo_headers: PseudoHeaders<'a>,

    /// headers
    headers: Cow<'a, Headers>,
}

impl<'a> FieldSection<'a> {
    /// Construct a new borrowed`FieldSection` for encoding
    pub fn new(pseudo_headers: PseudoHeaders<'a>, headers: &'a Headers) -> Self {
        Self {
            pseudo_headers,
            headers: Cow::Borrowed(headers),
        }
    }

    /// Decompose a `FieldSection` into pseudo headers and headers
    #[cfg(any(feature = "unstable", test))]
    pub fn into_parts(self) -> (PseudoHeaders<'a>, Headers) {
        (self.pseudo_headers, self.headers.into_owned())
    }
}

impl Display for FieldSection<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.pseudo_headers)?;
        for (n, v) in &*self.headers {
            for v in v {
                writeln!(f, "{n}: {v}")?;
            }
        }
        Ok(())
    }
}
