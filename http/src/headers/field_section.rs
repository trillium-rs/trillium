//! Protocol-agnostic representations of an HTTP field section.
//!
//! An HTTP field section is the combined pseudo-headers + regular headers payload of a
//! request or response. The same structure is decoded from HPACK (HTTP/2) and QPACK
//! (HTTP/3).
//!
//! [`FieldLineValue`] tracks provenance (static / borrowed / owned) of a value slice so the
//! encoder and dynamic-table code paths can defer cloning until the last possible moment.

use super::{
    Headers,
    entry_name::{EntryName, PseudoHeaderName},
    header_value::HeaderValueInner,
};
use crate::{Method, Status};
use fieldwork::Fieldwork;
use smallvec::SmallVec;
use smartcow::SmartCow;
use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
    hash,
    ops::Deref,
};

/// The six defined HTTP pseudo-header fields.
///
/// Unlike regular headers, pseudo-headers are a fixed set — unknown pseudo-headers are a
/// protocol error. Each may appear at most once.
#[derive(Debug, Default, Clone, PartialEq, Eq, Fieldwork)]
#[fieldwork(
    get,
    get_mut(option_borrow_inner = false),
    take,
    with,
    without,
    set,
    into
)]
pub struct PseudoHeaders<'a> {
    /// `:method` pseudo-header
    #[field(copy)]
    pub(in crate::headers) method: Option<Method>,

    /// `:status` pseudo-header
    #[field(copy)]
    pub(in crate::headers) status: Option<Status>,

    /// `:path` pseudo-header
    pub(in crate::headers) path: Option<Cow<'a, str>>,

    /// `:scheme` pseudo-header
    pub(in crate::headers) scheme: Option<Cow<'a, str>>,

    /// `:authority` pseudo-header
    pub(in crate::headers) authority: Option<Cow<'a, str>>,

    /// `:protocol` pseudo-header
    pub(in crate::headers) protocol: Option<Cow<'a, str>>,
}

impl PseudoHeaders<'_> {
    /// `true` when no pseudo-header fields are set.
    pub fn is_empty(&self) -> bool {
        self.method.is_none()
            && self.status.is_none()
            && self.path.is_none()
            && self.scheme.is_none()
            && self.authority.is_none()
            && self.protocol.is_none()
    }

    /// Convert into a `PseudoHeaders<'static>` by allocating any borrowed string fields.
    #[allow(
        dead_code,
        reason = "consumed by external callers; not visible in this crate's build"
    )]
    pub fn into_owned(self) -> PseudoHeaders<'static> {
        PseudoHeaders {
            method: self.method,
            status: self.status,
            path: self.path.map(|c| Cow::Owned(c.into_owned())),
            scheme: self.scheme.map(|c| Cow::Owned(c.into_owned())),
            authority: self.authority.map(|c| Cow::Owned(c.into_owned())),
            protocol: self.protocol.map(|c| Cow::Owned(c.into_owned())),
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

/// Combined [`PseudoHeaders`] and [`Headers`] — one HTTP field section.
#[derive(Debug, Clone, Fieldwork)]
#[fieldwork(get, get_mut, into_field)]
pub struct FieldSection<'a> {
    /// pseudo-headers
    pseudo_headers: PseudoHeaders<'a>,

    /// headers
    headers: Cow<'a, Headers>,
}

impl<'a> FieldSection<'a> {
    /// Construct a new borrowed `FieldSection` for encoding.
    pub fn new(pseudo_headers: PseudoHeaders<'a>, headers: &'a Headers) -> Self {
        Self {
            pseudo_headers,
            headers: Cow::Borrowed(headers),
        }
    }

    /// Construct a `FieldSection` owning its headers — used by decoders that produce a
    /// fresh [`Headers`] from the wire.
    pub(in crate::headers) fn from_owned(
        pseudo_headers: PseudoHeaders<'static>,
        headers: Headers,
    ) -> FieldSection<'static> {
        FieldSection {
            pseudo_headers,
            headers: Cow::Owned(headers),
        }
    }

    /// Flatten this field section into an ordered list of `(name, value, never_indexed)`
    /// triples suitable for feeding to a compression-aware encoder.
    ///
    /// Pseudo-headers come first in RFC-mandated order; regular headers follow.
    /// `FieldLineValue` provenance is preserved so a downstream encoder can elide
    /// allocations for already-static slices. The `never_indexed` flag carries the
    /// HPACK / QPACK N bit per value; pseudo-headers are always `false` because they
    /// round-trip through typed `Conn` fields, not the `Headers` map.
    pub(in crate::headers) fn field_lines(&self) -> FieldLines<'_> {
        fn field_line_value_from(v: &crate::HeaderValue) -> FieldLineValue<'_> {
            if let HeaderValueInner::Utf8(SmartCow::Borrowed(b)) = &v.inner {
                FieldLineValue::Static(b.as_bytes())
            } else {
                FieldLineValue::Borrowed(v.as_ref())
            }
        }

        // Inline capacity covers a typical response (`:status` + a handful of headers) with no
        // heap allocation; larger sections (e.g. proxied responses forwarding many headers)
        // spill to a single right-sized heap allocation via `with_capacity`.
        let mut lines = SmallVec::with_capacity(self.headers.len() + 6);
        if let Some(method) = &self.pseudo_headers.method {
            lines.push((
                PseudoHeaderName::Method.into(),
                FieldLineValue::Static(method.as_str().as_bytes()),
                false,
            ));
        }

        if let Some(status) = &self.pseudo_headers.status {
            lines.push((
                PseudoHeaderName::Status.into(),
                FieldLineValue::Static(status.code().as_bytes()),
                false,
            ));
        }

        if let Some(path) = &self.pseudo_headers.path {
            lines.push((
                PseudoHeaderName::Path.into(),
                FieldLineValue::Borrowed(path.as_bytes()),
                false,
            ));
        }
        if let Some(scheme) = &self.pseudo_headers.scheme {
            lines.push((
                PseudoHeaderName::Scheme.into(),
                FieldLineValue::Borrowed(scheme.as_bytes()),
                false,
            ));
        }

        if let Some(authority) = &self.pseudo_headers.authority {
            lines.push((
                PseudoHeaderName::Authority.into(),
                FieldLineValue::Borrowed(authority.as_bytes()),
                false,
            ));
        }

        if let Some(protocol) = &self.pseudo_headers.protocol {
            lines.push((
                PseudoHeaderName::Protocol.into(),
                FieldLineValue::Borrowed(protocol.as_bytes()),
                false,
            ));
        }

        // Iterate the inner maps directly (rather than the public `Iter`) so the
        // `UnknownHeaderName<'static>` inner lifetime is preserved on each item;
        // `Iter` erases it to the iterator's borrow lifetime, which would prevent
        // calling `into_lower_static`.
        for (k, hv) in &self.headers.known {
            for v in hv {
                let value = field_line_value_from(v);
                lines.push((EntryName::Known(*k), value, v.is_never_indexed()));
            }
        }

        for (uhn, hv) in &self.headers.unknown {
            for v in hv {
                let value = field_line_value_from(v);
                // Route the clone through the lowercase interner so any recoverable
                // `&'static str` survives lifetime erasure via the `UnknownStatic`
                // variant tag.
                let lowered = uhn.clone().into_lower_static();
                let name = match lowered.as_static_str() {
                    Some(s) => EntryName::UnknownStatic(s),
                    None => EntryName::Unknown(lowered),
                };
                lines.push((name, value, v.is_never_indexed()));
            }
        }

        lines
    }

    /// Decompose a `FieldSection` into its pseudo-headers and headers.
    pub fn into_parts(self) -> (PseudoHeaders<'a>, Headers) {
        (self.pseudo_headers, self.headers.into_owned())
    }

    /// The *uncompressed* size of this field section: the sum, over every field line, of the
    /// name's length in bytes, the value's length in bytes, and a 32-byte per-field overhead.
    /// Pseudo-header names count their leading colon (`:method` is 7).
    ///
    /// This is the metric both HTTP/2's `SETTINGS_MAX_HEADER_LIST_SIZE` ([RFC 7540 §6.5.2]) and
    /// HTTP/3's `SETTINGS_MAX_FIELD_SECTION_SIZE` ([RFC 9114 §4.2.2]) are defined in — the 32-byte
    /// overhead and the formula are identical, both deriving from the HPACK entry size
    /// ([RFC 7541 §4.1]). It is independent of HPACK/QPACK compression, which is why it can't be
    /// read off the encoded byte length.
    ///
    /// [RFC 7540 §6.5.2]: https://www.rfc-editor.org/rfc/rfc7540#section-6.5.2
    /// [RFC 9114 §4.2.2]: https://www.rfc-editor.org/rfc/rfc9114#section-4.2.2
    /// [RFC 7541 §4.1]: https://www.rfc-editor.org/rfc/rfc7541#section-4.1
    pub(crate) fn uncompressed_len(&self) -> u64 {
        self.field_lines()
            .iter()
            .map(|(name, value, _)| name.len() as u64 + value.len() as u64 + 32)
            .sum()
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

/// An ordered list of `(name, value, never_indexed)` field-line triples, as produced by
/// [`FieldSection::field_lines`] for a compression-aware encoder.
///
/// Inline storage holds 16 lines — enough for a typical response field section to stay on
/// the stack — and spills to the heap beyond that.
pub(in crate::headers) type FieldLines<'a> =
    SmallVec<[(EntryName<'a>, FieldLineValue<'a>, bool); 16]>;

/// A byte-slice value that tracks its provenance — static, externally borrowed, or owned.
///
/// Serves the same purpose as `Cow<'a, Cow<'static, [u8]>>` but with a cleaner surface. The
/// `Static` variant lets us keep static literals cheap through the whole encode path;
/// `Borrowed` lets a decoder yield zero-copy slices into the frame buffer; `Owned` is the
/// escape hatch for Huffman-decoded bytes and similar transforms.
///
/// `PartialEq` / `Eq` / `Hash` delegate to the underlying bytes — provenance is a storage
/// detail, not a semantic distinction.
#[derive(Debug, Clone)]
pub(crate) enum FieldLineValue<'a> {
    Static(&'static [u8]),
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

impl Deref for FieldLineValue<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_bytes()
    }
}

impl PartialEq for FieldLineValue<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.as_bytes() == other.as_bytes()
    }
}

impl Eq for FieldLineValue<'_> {}

impl hash::Hash for FieldLineValue<'_> {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.as_bytes().hash(state);
    }
}

impl FieldLineValue<'_> {
    pub(in crate::headers) fn into_static(self) -> Cow<'static, [u8]> {
        match self {
            FieldLineValue::Static(b) => Cow::Borrowed(b),
            FieldLineValue::Borrowed(b) => Cow::Owned(b.to_vec()),
            FieldLineValue::Owned(b) => Cow::Owned(b),
        }
    }

    pub(in crate::headers) fn reborrow(&self) -> FieldLineValue<'_> {
        match self {
            FieldLineValue::Static(items) => FieldLineValue::Static(items),
            FieldLineValue::Borrowed(items) => FieldLineValue::Borrowed(items),
            FieldLineValue::Owned(items) => FieldLineValue::Borrowed(items),
        }
    }

    pub(in crate::headers) fn as_bytes(&self) -> &[u8] {
        match self {
            FieldLineValue::Static(items) | FieldLineValue::Borrowed(items) => items,
            FieldLineValue::Owned(items) => items,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KnownHeaderName, Method};

    #[test]
    fn uncompressed_len_sums_name_value_plus_32_per_field() {
        // :method GET  -> 7 + 3 + 32 = 42
        // accept: */*  -> 6 + 3 + 32 = 41
        let mut headers = Headers::new();
        headers.insert(KnownHeaderName::Accept, "*/*");
        let pseudo = PseudoHeaders::default().with_method(Method::Get);
        let field_section = FieldSection::new(pseudo, &headers);
        assert_eq!(field_section.uncompressed_len(), 42 + 41);
    }

    #[test]
    fn uncompressed_len_counts_repeated_values_separately() {
        // two set-cookie values each count as their own field line
        let mut headers = Headers::new();
        headers.append(KnownHeaderName::SetCookie, "a=1"); // 10 + 3 + 32 = 45
        headers.append(KnownHeaderName::SetCookie, "bb=22"); // 10 + 5 + 32 = 47
        let field_section = FieldSection::new(PseudoHeaders::default(), &headers);
        assert_eq!(field_section.uncompressed_len(), 45 + 47);
    }
}
