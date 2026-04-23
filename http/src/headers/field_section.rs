//! Protocol-agnostic representations of an HTTP field section.
//!
//! An HTTP field section is the combined pseudo-headers + regular headers payload of a
//! request or response. The same structure is decoded from HPACK (HTTP/2) and QPACK
//! (HTTP/3); moving it here (out of [`crate::headers::qpack`]) lets HPACK share the type
//! verbatim when it lands.
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
use smartcow::SmartCow;
use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
    hash,
    ops::Deref,
};

/// The six defined HTTP pseudo-header fields (RFC 9113 §8.3, RFC 9114 §4.3, RFC 9220).
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
    /// `true` when no pseudo-header fields are set. Useful for validating trailer sections,
    /// which per RFC 9113 §8.1 (h2) and RFC 9114 §4.4 (h3) MUST NOT include pseudo-headers.
    pub fn is_empty(&self) -> bool {
        self.method.is_none()
            && self.status.is_none()
            && self.path.is_none()
            && self.scheme.is_none()
            && self.authority.is_none()
            && self.protocol.is_none()
    }

    /// Convert into a `PseudoHeaders<'static>` by allocating any borrowed string fields.
    /// Used by callers that need to move pseudo-headers across an async boundary that
    /// outlives the original borrow.
    #[allow(
        dead_code,
        reason = "consumed by trillium-client; not visible in this crate's build"
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

    /// Flatten this field section into an ordered list of `(name, value)` pairs suitable
    /// for feeding to a compression-aware encoder.
    ///
    /// Pseudo-headers come first in RFC-mandated order; regular headers follow.
    /// `FieldLineValue` provenance is preserved so a downstream encoder can elide
    /// allocations for already-static slices.
    pub(in crate::headers) fn field_lines(&self) -> Vec<(EntryName<'_>, FieldLineValue<'_>)> {
        fn field_line_value_from(v: &crate::HeaderValue) -> FieldLineValue<'_> {
            if let HeaderValueInner::Utf8(SmartCow::Borrowed(b)) = &v.0 {
                FieldLineValue::Static(b.as_bytes())
            } else {
                FieldLineValue::Borrowed(v.as_ref())
            }
        }

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

        // Iterate the inner maps directly (rather than the public `Iter`) so the
        // `UnknownHeaderName<'static>` inner lifetime is preserved on each item;
        // `Iter` erases it to the iterator's borrow lifetime, which would prevent
        // calling `into_lower_static`.
        for (k, hv) in &self.headers.known {
            for v in hv {
                let value = field_line_value_from(v);
                lines.push((EntryName::Known(*k), value));
            }
        }

        for (uhn, hv) in &self.headers.unknown {
            for v in hv {
                let value = field_line_value_from(v);
                // Headers stores `UnknownHeaderName<'static>`, so route the clone
                // through the lowercase interner — uppercase literals get
                // interned to a canonical `&'static str` and uppercase Owned
                // names allocate a lowercased copy. The resulting
                // `&'static str` (when present) survives lifetime erasure into
                // `Vec<(EntryName<'_>, _)>` via the `UnknownStatic`
                // variant tag, regardless of what '_ is.
                let lowered = uhn.clone().into_lower_static();
                let name = match lowered.as_static_str() {
                    Some(s) => EntryName::UnknownStatic(s),
                    None => EntryName::Unknown(lowered),
                };
                lines.push((name, value));
            }
        }

        lines
    }

    /// Decompose a `FieldSection` into its pseudo-headers and headers.
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
