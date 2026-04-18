//! QPACK types
//!
//! Please note that this interface is likely to change

// The outbound dynamic-table encode path is under active construction: several helpers and
// policy building blocks are intentionally present ahead of their first call site. Suppress
// dead-code warnings for the whole module until the encode strategies land.
#![allow(dead_code)]

#[cfg(test)]
mod decoder_corpus_tests;
mod decoder_dynamic_table;
#[cfg(test)]
mod encoder_corpus_tests;
mod encoder_dynamic_table;
mod entry_name;
pub(crate) mod huffman;
mod instruction;
#[cfg(test)]
mod qif;
pub(crate) mod static_table;
#[cfg(test)]
mod tests;
pub(crate) mod varint;

// Wire-format constants for §4.5 field sections live in `instruction::field_section`.
// Encoder-stream instruction constants (§3.2) live in `instruction::encoder`.
// Decoder-stream instruction constants (§4.4) live in `instruction::decoder`.
// §4.1.2 string-literal encoding helpers live in `instruction` (module-level).
use super::header_value::HeaderValueInner;
use crate::{
    Headers, Method, Status,
    h3::{H3Error, H3ErrorCode},
    headers::qpack::{
        entry_name::QpackEntryName, static_table::PseudoHeaderName, varint::VarIntError,
    },
};
#[cfg(not(feature = "unstable"))]
pub(crate) use decoder_dynamic_table::DecoderDynamicTable;
#[cfg(feature = "unstable")]
pub use decoder_dynamic_table::DecoderDynamicTable;
#[cfg(not(feature = "unstable"))]
pub(crate) use encoder_dynamic_table::EncoderDynamicTable;
#[cfg(feature = "unstable")]
pub use encoder_dynamic_table::EncoderDynamicTable;
use fieldwork::Fieldwork;
#[cfg(feature = "unstable")]
pub use huffman::HuffmanError;
#[cfg(not(feature = "unstable"))]
use huffman::HuffmanError;
use smartcow::SmartCow;
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

impl PseudoHeaders<'_> {
    fn get(&self, pseudo_header_name: PseudoHeaderName) -> Option<&str> {
        match pseudo_header_name {
            PseudoHeaderName::Authority => self.authority.as_deref(),
            PseudoHeaderName::Method => self.method.map(|m| m.as_str()),
            PseudoHeaderName::Path => self.path.as_deref(),
            PseudoHeaderName::Protocol => self.protocol.as_deref(),
            PseudoHeaderName::Scheme => self.scheme.as_deref(),
            PseudoHeaderName::Status => self.status.map(|s| s.code()),
        }
    }
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

/// A type so that we keep track of whether we're borrowing content from a local lifetime (borrowed
/// headers), have a static vec, or directly own the content.  This type exists specifically to
/// defer cloning/allocating until we're absolutely sure we're going to need to do so (there are
/// many paths that only ever need a borrowed slice). If we pass &[u8], we lose track of whether the
/// livetime is static, in which case we wouldn't need to clone at all. This is effectively a
/// Cow<'a, Cow<'static, [u8]>>
#[derive(Debug, Clone)]
pub(crate) enum FieldLineValue<'a> {
    Static(&'static [u8]),
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

impl std::ops::Deref for FieldLineValue<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}

/// Equality delegates to [`as_bytes`](Self::as_bytes): `Static(b"x")`, `Borrowed(b"x")`, and
/// `Owned(b"x".to_vec())` all compare equal. Provenance (static / borrowed / owned) is a
/// storage detail, not a semantic distinction.
impl PartialEq for FieldLineValue<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for FieldLineValue<'_> {}

impl FieldLineValue<'_> {
    fn into_static(self) -> Cow<'static, [u8]> {
        match self {
            FieldLineValue::Static(b) => Cow::Borrowed(b),
            FieldLineValue::Borrowed(b) => Cow::Owned(b.to_vec()),
            FieldLineValue::Owned(b) => Cow::Owned(b),
        }
    }

    fn as_static(&self) -> Cow<'static, [u8]> {
        match self {
            FieldLineValue::Static(b) => Cow::Borrowed(b),
            FieldLineValue::Borrowed(b) => Cow::Owned(b.to_vec()),
            FieldLineValue::Owned(b) => Cow::Owned(b.clone()),
        }
    }

    fn reborrow(&self) -> FieldLineValue<'_> {
        match self {
            FieldLineValue::Static(items) => FieldLineValue::Static(items),
            FieldLineValue::Borrowed(items) => FieldLineValue::Borrowed(items),
            FieldLineValue::Owned(items) => FieldLineValue::Borrowed(items),
        }
    }

    fn as_bytes(&self) -> &[u8] {
        match self {
            FieldLineValue::Static(items) | FieldLineValue::Borrowed(items) => items,
            FieldLineValue::Owned(items) => items,
        }
    }
}

impl<'a> FieldSection<'a> {
    /// Construct a new borrowed `FieldSection` for encoding
    pub fn new(pseudo_headers: PseudoHeaders<'a>, headers: &'a Headers) -> Self {
        Self {
            pseudo_headers,
            headers: Cow::Borrowed(headers),
        }
    }

    fn field_lines(&self) -> Vec<(QpackEntryName<'_>, FieldLineValue<'_>)> {
        let mut lines = Vec::with_capacity(self.headers.len() + 6);
        if let Some(method) = &self.pseudo_headers.method {
            lines.push((
                PseudoHeaderName::Method.into(),
                FieldLineValue::Static(method.as_str().as_bytes()),
            ));
        }

        if let Some(status) = &self.pseudo_headers.status {
            lines.push((
                PseudoHeaderName::Status.into(),
                FieldLineValue::Static(status.code().as_bytes()),
            ));
        }

        if let Some(path) = &self.pseudo_headers.path {
            lines.push((
                PseudoHeaderName::Path.into(),
                FieldLineValue::Borrowed(path.as_bytes()),
            ));
        }
        if let Some(scheme) = &self.pseudo_headers.scheme {
            lines.push((
                PseudoHeaderName::Scheme.into(),
                FieldLineValue::Borrowed(scheme.as_bytes()),
            ));
        }

        if let Some(authority) = &self.pseudo_headers.authority {
            lines.push((
                PseudoHeaderName::Authority.into(),
                FieldLineValue::Borrowed(authority.as_bytes()),
            ));
        }

        if let Some(protocol) = &self.pseudo_headers.protocol {
            lines.push((
                PseudoHeaderName::Protocol.into(),
                FieldLineValue::Borrowed(protocol.as_bytes()),
            ));
        }

        for (hn, hv) in &*self.headers {
            for v in hv {
                let v = if let HeaderValueInner::Utf8(SmartCow::Borrowed(b)) = &v.0 {
                    FieldLineValue::Static(b.as_bytes())
                } else {
                    FieldLineValue::Borrowed(v.as_ref())
                };
                lines.push((QpackEntryName::from(hn.clone()), v));
            }
        }

        lines
    }

    /// Decompose a `FieldSection` into pseudo headers and headers
    #[cfg(test)]
    pub fn into_parts(self) -> (PseudoHeaders<'a>, Headers) {
        (self.pseudo_headers, self.headers.into_owned())
    }

    fn get(&self, entry_name: &QpackEntryName) -> Option<&str> {
        match entry_name {
            QpackEntryName::Known(k) => self.headers.get_str(*k),
            QpackEntryName::Unknown(u) => self.headers.get_str(u),
            QpackEntryName::Pseudo(p) => self.pseudo_headers.get(*p),
        }
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

/// Errors that can occur during QPACK decoding.
#[derive(Debug, thiserror::Error, Clone, Copy)]
pub(crate) enum QpackError {
    #[error(transparent)]
    Huffman(#[from] HuffmanError),

    #[error(transparent)]
    VarInt(#[from] VarIntError),

    #[error("static table index {0} out of range (0-98)")]
    InvalidStaticIndex(usize),

    #[error("unexpected end of field section")]
    UnexpectedEnd,

    #[error("invalid header name")]
    InvalidHeaderName,

    #[error("invalid header value")]
    InvalidHeaderValue,

    #[error("method not recongized")]
    UnrecognizedMethod,

    #[error("invalid status")]
    InvalidStatus,
}

impl From<QpackError> for H3Error {
    fn from(_: QpackError) -> Self {
        H3ErrorCode::QpackDecompressionFailed.into()
    }
}
