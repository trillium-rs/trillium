//! QPACK types
//!
//! Please note that this interface is likely to change

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

use crate::{Headers, Method, Status};
use fieldwork::Fieldwork;
use std::borrow::Cow;

/// The six defined HTTP/3 pseudo-header fields (RFC 9114 §4.3, RFC 9220).
///
/// Unlike regular headers, pseudo-headers are a fixed set — unknown
/// pseudo-headers are a protocol error. Each may appear at most once.
#[derive(Debug, Default, Clone, PartialEq, Eq, Fieldwork)]
#[fieldwork(get, take, with(into), without, set)]
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

pub use huffman::HuffmanError;

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

    /// Decompose a FieldSection into pseudo headers and headers
    pub fn into_parts(self) -> (PseudoHeaders<'a>, Headers) {
        (self.pseudo_headers, self.headers.into_owned())
    }
}
