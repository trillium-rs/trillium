//! A name as it can appear in an HPACK or QPACK dynamic table entry.
//!
//! [`EntryName`] is a superset of [`StaticHeaderName`]: it admits arbitrary unknown
//! header names (for entries inserted via Insert With Literal Name or dynamic-name
//! references) in addition to the known-header and pseudo-header variants that the static
//! table contains.
//!
//! Pseudo-headers never reach the dynamic table via a *literal* insert (the static name
//! reference is always strictly cheaper on the wire), but they can arrive via Insert With
//! Name Reference against a static pseudo-header slot, and propagate further via Duplicate
//! and Insert With Name Reference (dynamic).
use crate::{
    HeaderName, KnownHeaderName,
    h3::H3ErrorCode,
    headers::{HeaderNameInner, UnknownHeaderName, header_observer::NameKey},
};
use std::{
    fmt::{self, Debug, Display, Formatter},
    hash::{Hash, Hasher},
};

/// A dynamic table entry's name — either a known/pseudo-header (sealed enums)
/// or a regular unknown header name. Unknown names are split into two variants:
/// `UnknownStatic` for names recoverable as `&'static str` (literals normalized
/// through the lowercase interner), and `Unknown` for everything else (owned, or
/// borrowed-non-static). The split lets the encoder hot path query "is this name
/// static?" without checking lifetimes.
///
/// Equality and hashing compare by [`as_str`](Self::as_str) — the storage variant
/// is purely an indexing-into-priming-eligibility distinction, not a content one.
/// `Unknown("x-custom")`, `UnknownStatic("x-custom")` collide as map keys; only
/// the variant tag determines [`name_key`](Self::name_key) eligibility.
pub(in crate::headers) enum EntryName<'a> {
    Known(KnownHeaderName),
    /// An HTTP pseudo-header name (e.g. `:method`, `:path`).
    Pseudo(PseudoHeaderName),
    /// An unknown header name backed by a `&'static str` (typically a literal,
    /// possibly re-interned to its lowercased form). Eligible for cross-connection
    /// QPACK observer tracking.
    UnknownStatic(&'static str),
    /// An unknown header name backed by a non-static reference or owned storage.
    /// Not tracked cross-connection.
    Unknown(UnknownHeaderName<'a>),
}

impl Debug for EntryName<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(arg0) => write!(f, "{arg0}"),
            Self::Pseudo(arg0) => write!(f, "{arg0}"),
            Self::UnknownStatic(arg0) => write!(f, "&'static {arg0:?}"),
            Self::Unknown(arg0) => write!(f, "{arg0:?}"),
        }
    }
}

impl PartialEq for EntryName<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for EntryName<'_> {}

impl Hash for EntryName<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl Clone for EntryName<'static> {
    fn clone(&self) -> Self {
        match self {
            Self::Known(k) => Self::Known(*k),
            Self::Pseudo(p) => Self::Pseudo(*p),
            Self::UnknownStatic(s) => Self::UnknownStatic(s),
            Self::Unknown(u) => Self::Unknown(u.clone()),
        }
    }
}

impl EntryName<'_> {
    /// wire bytes for this entry name
    pub(in crate::headers) fn as_bytes(&self) -> &[u8] {
        self.as_ref().as_bytes()
    }

    pub(in crate::headers) fn as_str(&self) -> &str {
        match self {
            Self::Known(k) => k.as_lower_str(),
            Self::Pseudo(p) => p.as_str(),
            Self::UnknownStatic(s) => s,
            Self::Unknown(u) => u,
        }
    }

    /// Length in bytes of the name. Equivalent for the wire form and the natural form since
    /// ASCII case conversion preserves length. Used for entry-size calculation
    /// (RFC 9204 §3.2.1).
    pub(in crate::headers) fn len(&self) -> usize {
        self.as_bytes().len()
    }

    pub(in crate::headers) fn reborrow(&self) -> EntryName<'_> {
        match self {
            EntryName::Known(k) => EntryName::Known(*k),
            EntryName::Pseudo(p) => EntryName::Pseudo(*p),
            EntryName::UnknownStatic(s) => EntryName::UnknownStatic(s),
            EntryName::Unknown(u) => EntryName::Unknown(u.reborrow()),
        }
    }

    pub(in crate::headers) fn into_owned(self) -> EntryName<'static> {
        match self {
            EntryName::Known(k) => EntryName::Known(k),
            EntryName::Pseudo(p) => EntryName::Pseudo(p),
            EntryName::UnknownStatic(s) => EntryName::UnknownStatic(s),
            EntryName::Unknown(u) => EntryName::Unknown(u.into_owned()),
        }
    }

    /// Stable, content-equal observer key for this name, if eligible for
    /// cross-connection priming. Returns `Some` only for variants whose value
    /// is known to be program-controlled by construction
    /// ([`Known`](Self::Known), [`Pseudo`](Self::Pseudo), and
    /// [`UnknownStatic`](Self::UnknownStatic) — the latter is `&'static str`
    /// because it came from a literal and was passed through the lowercase
    /// interner). Returns `None` for [`Unknown`](Self::Unknown), whose contents
    /// can be user-derived.
    pub(in crate::headers) fn name_key(&self) -> Option<NameKey> {
        match self {
            Self::Known(k) => Some(NameKey::Known(*k)),
            Self::Pseudo(p) => Some(NameKey::Pseudo(*p)),
            Self::UnknownStatic(s) => Some(NameKey::UnknownStatic(s)),
            Self::Unknown(_) => None,
        }
    }

    /// True if the *value* under this name must never reach a dynamic table for
    /// privacy reasons — caching would let a CRIME-style length side-channel against a
    /// shared dynamic table learn secret values.
    ///
    /// Stand-in for propagating the RFC 9204 §4.5.4 N ("Never Indexed") bit through
    /// trillium-proxy; see the `qpack-n-bit-gap` memory note.
    ///
    /// This predicate is a *ban* — callers that additionally want to skip names whose
    /// caching is merely *unprofitable* (e.g. `date`, whose rapidly-changing value the
    /// header observer filters on the cost-model side) should add their own check on top
    /// rather than widening this list.
    pub(in crate::headers) fn has_uncacheable_value(&self) -> bool {
        matches!(
            self,
            EntryName::Known(
                KnownHeaderName::Authorization
                    | KnownHeaderName::Cookie
                    | KnownHeaderName::SetCookie
                    | KnownHeaderName::ProxyAuthorization
                    | KnownHeaderName::AuthenticationInfo
            )
        )
    }
}

impl EntryName<'static> {
    fn known_or_pseudo(bytes: &[u8]) -> Result<Option<Self>, H3ErrorCode> {
        if bytes.is_empty() {
            log::error!("QPACK encoder: empty qpack entry name");
            return Err(H3ErrorCode::QpackEncoderStreamError);
        }

        if bytes.first() == Some(&b':') {
            let pseudo = PseudoHeaderName::lowercase_byte_match(bytes).ok_or_else(|| {
                log::error!(
                    "QPACK encoder: unknown pseudo-header in literal name: {:?}",
                    String::from_utf8_lossy(bytes)
                );
                H3ErrorCode::QpackEncoderStreamError
            })?;
            Ok(Some(EntryName::from(pseudo)))
        } else if let Some(khn) = KnownHeaderName::lowercase_byte_match(bytes) {
            Ok(Some(EntryName::from(khn)))
        } else {
            Ok(None)
        }
    }
}

impl<'a> TryFrom<&'a [u8]> for EntryName<'a> {
    // Note: error type is H3-flavored while this type is being shared cross-protocol. HPACK
    // will map the same conditions to COMPRESSION_ERROR; a neutral error type can come later
    // if the callsite mapping gets awkward.
    type Error = H3ErrorCode;

    fn try_from(name_bytes: &'a [u8]) -> Result<Self, Self::Error> {
        if let Some(qen) = EntryName::known_or_pseudo(name_bytes)? {
            return Ok(qen);
        }

        let str = std::str::from_utf8(name_bytes).map_err(|_| {
            log::error!(
                "QPACK encoder: non-utf8 header name {:?}",
                String::from_utf8_lossy(name_bytes)
            );
            H3ErrorCode::QpackEncoderStreamError
        })?;

        let uhn = UnknownHeaderName::from(str);

        if !uhn.is_valid_lower() {
            log::error!("QPACK encoder: non-lower-ascii header name {uhn:?}");
            return Err(H3ErrorCode::QpackEncoderStreamError);
        }

        Ok(EntryName::Unknown(uhn))
    }
}

impl TryFrom<Vec<u8>> for EntryName<'static> {
    type Error = H3ErrorCode;

    fn try_from(name_bytes: Vec<u8>) -> Result<Self, Self::Error> {
        if let Some(qen) = EntryName::known_or_pseudo(&name_bytes)? {
            return Ok(qen);
        }

        let string = String::from_utf8(name_bytes).map_err(|e| {
            log::error!(
                "QPACK encoder: bytes were not a string: {}",
                String::from_utf8_lossy(e.as_bytes())
            );
            H3ErrorCode::QpackEncoderStreamError
        })?;

        let uhn = UnknownHeaderName::from(string);

        if !uhn.is_valid_lower() {
            log::error!("QPACK encoder: non-lower-ascii header name {uhn:?}");
            return Err(H3ErrorCode::QpackEncoderStreamError);
        }

        Ok(EntryName::Unknown(uhn))
    }
}

impl AsRef<str> for EntryName<'_> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Display for EntryName<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(k) => write!(f, "{k}"),
            Self::Pseudo(p) => write!(f, "{p}"),
            Self::UnknownStatic(s) => f.write_str(s),
            Self::Unknown(u) => write!(f, "{u}"),
        }
    }
}

impl<'a> From<HeaderName<'a>> for EntryName<'a> {
    fn from(h: HeaderName<'a>) -> Self {
        match h.0 {
            HeaderNameInner::KnownHeader(k) => Self::Known(k),
            HeaderNameInner::UnknownHeader(u) => Self::Unknown(u.into_lower()),
        }
    }
}

impl<'a> From<&'a HeaderName<'_>> for EntryName<'a> {
    fn from(h: &'a HeaderName<'_>) -> Self {
        match &h.0 {
            HeaderNameInner::KnownHeader(k) => Self::Known(*k),
            HeaderNameInner::UnknownHeader(u) => Self::Unknown(u.reborrow().into_lower()),
        }
    }
}

impl From<KnownHeaderName> for EntryName<'static> {
    fn from(value: KnownHeaderName) -> Self {
        Self::Known(value)
    }
}

impl From<PseudoHeaderName> for EntryName<'static> {
    fn from(value: PseudoHeaderName) -> Self {
        Self::Pseudo(value)
    }
}

/// The HTTP pseudo-header names (RFC 9113 §8.3 / RFC 9114 §4.3, extended by RFC 9220).
///
/// These form a closed set; an unknown `:foo` on the wire is a protocol error.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
#[repr(u8)]
pub(in crate::headers) enum PseudoHeaderName {
    Authority,
    Method,
    Path,
    Protocol,
    Scheme,
    Status,
}

impl PseudoHeaderName {
    #[cfg(test)]
    pub(in crate::headers) const VARIANTS: &[PseudoHeaderName] = &[
        Self::Authority,
        Self::Method,
        Self::Path,
        Self::Protocol,
        Self::Scheme,
        Self::Status,
    ];

    /// Retrieve a `'static str` representation (e.g. `":method"`).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Authority => ":authority",
            Self::Method => ":method",
            Self::Path => ":path",
            Self::Protocol => ":protocol",
            Self::Scheme => ":scheme",
            Self::Status => ":status",
        }
    }

    pub(in crate::headers) fn lowercase_byte_match(bytes: &[u8]) -> Option<Self> {
        match bytes {
            b":authority" => Some(Self::Authority),
            b":method" => Some(Self::Method),
            b":path" => Some(Self::Path),
            b":protocol" => Some(Self::Protocol),
            b":scheme" => Some(Self::Scheme),
            b":status" => Some(Self::Status),
            _ => None,
        }
    }
}

impl Display for PseudoHeaderName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{EntryName, PseudoHeaderName};
    use crate::{HeaderName, KnownHeaderName};

    #[test]
    fn khn_round_trip() {
        for khn in KnownHeaderName::VARIANTS {
            assert_eq!(
                EntryName::try_from(khn.as_lower_str().as_bytes()).unwrap(),
                EntryName::from(*khn)
            );
            assert_eq!(
                EntryName::try_from(khn.as_lower_str().as_bytes().to_owned()).unwrap(),
                EntryName::from(*khn)
            );
        }
    }

    #[test]
    fn pseudo_round_trip() {
        for khn in KnownHeaderName::VARIANTS {
            assert_eq!(
                EntryName::try_from(khn.as_lower_str().as_bytes()).unwrap(),
                EntryName::from(*khn)
            );
            assert_eq!(
                EntryName::try_from(khn.as_lower_str().as_bytes().to_owned()).unwrap(),
                EntryName::from(*khn)
            );
        }
    }

    #[test]
    fn other_round_trip() {
        assert_eq!(
            EntryName::try_from(b"x-other".as_slice()).unwrap(),
            EntryName::from(HeaderName::from("x-other"))
        );

        assert_eq!(
            EntryName::try_from(b"x-other".to_vec()).unwrap(),
            EntryName::from(HeaderName::from("x-other"))
        );
    }

    #[test]
    fn unknown_has_no_name_key() {
        let unknown = EntryName::try_from(b"x-other".as_slice()).unwrap();
        assert_eq!(unknown.name_key(), None);
    }

    #[test]
    fn pseudo_header_name_round_trip() {
        for phn in PseudoHeaderName::VARIANTS.iter().copied() {
            assert_eq!(
                PseudoHeaderName::lowercase_byte_match(phn.as_str().as_bytes()),
                Some(phn)
            );
        }
        assert!(PseudoHeaderName::lowercase_byte_match(b":other").is_none());
    }
}
