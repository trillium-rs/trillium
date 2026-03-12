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
use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
};

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

#[cfg(feature = "unstable")]
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
