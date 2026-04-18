//! A name as it can appear in a QPACK dynamic table entry.
//!
//! [`QpackEntryName`] is a superset of [`StaticHeaderName`]: it admits arbitrary unknown
//! header names (for entries inserted via Insert With Literal Name or dynamic-name
//! references) in addition to the known-header and pseudo-header variants that the static
//! table contains.
//!
//! Pseudo-headers never reach the dynamic table via a *literal* insert (the static name
//! reference is always strictly cheaper on the wire), but they can arrive via Insert With
//! Name Reference against a static pseudo-header slot, and propagate further via Duplicate
//! and Insert With Name Reference (dynamic).
use super::static_table::{PseudoHeaderName, StaticHeaderName};
use crate::{
    HeaderName, KnownHeaderName,
    h3::H3ErrorCode,
    headers::{HeaderNameInner, UnknownHeaderName},
};
use std::fmt;

/// A QPACK dynamic table entry's name — either a regular header name or a pseudo-header.
#[derive(Debug, Eq, PartialEq, Hash)]
pub(in crate::headers) enum QpackEntryName<'a> {
    Known(KnownHeaderName),
    /// A regular header name
    Unknown(UnknownHeaderName<'a>),
    /// An HTTP pseudo-header name (e.g. `:method`, `:path`).
    Pseudo(PseudoHeaderName),
}

impl Clone for QpackEntryName<'static> {
    fn clone(&self) -> Self {
        match self {
            Self::Known(k) => Self::Known(*k),
            Self::Unknown(u) => Self::Unknown(u.clone()),
            Self::Pseudo(p) => Self::Pseudo(*p),
        }
    }
}

impl QpackEntryName<'_> {
    /// wire bytes for this entry name
    pub(in crate::headers) fn as_bytes(&self) -> &[u8] {
        self.as_ref().as_bytes()
    }

    pub(in crate::headers) fn as_str(&self) -> &str {
        match self {
            Self::Known(k) => k.as_lower_str(),
            Self::Unknown(u) => u,
            Self::Pseudo(p) => p.as_str(),
        }
    }

    /// Length in bytes of the name. Equivalent for the wire form and the natural form since
    /// ASCII case conversion preserves length. Used for entry-size calculation
    /// (RFC 9204 §3.2.1).
    pub(in crate::headers) fn len(&self) -> usize {
        self.as_bytes().len()
    }

    pub(in crate::headers) fn reborrow(&self) -> QpackEntryName<'_> {
        match self {
            QpackEntryName::Known(k) => QpackEntryName::Known(*k),
            QpackEntryName::Unknown(u) => QpackEntryName::Unknown(u.reborrow()),
            QpackEntryName::Pseudo(p) => QpackEntryName::Pseudo(*p),
        }
    }

    pub(in crate::headers) fn into_owned(self) -> QpackEntryName<'static> {
        match self {
            QpackEntryName::Known(k) => QpackEntryName::Known(k),
            QpackEntryName::Unknown(u) => QpackEntryName::Unknown(u.into_owned()),
            QpackEntryName::Pseudo(p) => QpackEntryName::Pseudo(p),
        }
    }
}

impl QpackEntryName<'static> {
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
            Ok(Some(QpackEntryName::from(pseudo)))
        } else if let Some(khn) = KnownHeaderName::lowercase_byte_match(bytes) {
            Ok(Some(QpackEntryName::from(khn)))
        } else {
            Ok(None)
        }
    }
}

impl<'a> TryFrom<&'a [u8]> for QpackEntryName<'a> {
    type Error = H3ErrorCode;

    fn try_from(name_bytes: &'a [u8]) -> Result<Self, Self::Error> {
        if let Some(qen) = QpackEntryName::known_or_pseudo(name_bytes)? {
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

        Ok(QpackEntryName::Unknown(uhn))
    }
}

impl TryFrom<Vec<u8>> for QpackEntryName<'static> {
    type Error = H3ErrorCode;

    fn try_from(name_bytes: Vec<u8>) -> Result<Self, Self::Error> {
        if let Some(qen) = QpackEntryName::known_or_pseudo(&name_bytes)? {
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

        Ok(QpackEntryName::Unknown(uhn))
    }
}

impl AsRef<str> for QpackEntryName<'_> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for QpackEntryName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(k) => write!(f, "{k}"),
            Self::Unknown(u) => write!(f, "{u}"),
            Self::Pseudo(p) => write!(f, "{p}"),
        }
    }
}

impl From<StaticHeaderName> for QpackEntryName<'static> {
    fn from(s: StaticHeaderName) -> Self {
        match s {
            StaticHeaderName::Header(known) => known.into(),
            StaticHeaderName::Pseudo(pseudo) => pseudo.into(),
        }
    }
}

impl<'a> From<HeaderName<'a>> for QpackEntryName<'a> {
    fn from(h: HeaderName<'a>) -> Self {
        match h.0 {
            HeaderNameInner::KnownHeader(k) => Self::Known(k),
            HeaderNameInner::UnknownHeader(u) => Self::Unknown(u.into_lower()),
        }
    }
}

impl<'a> From<&'a HeaderName<'_>> for QpackEntryName<'a> {
    fn from(h: &'a HeaderName<'_>) -> Self {
        match &h.0 {
            HeaderNameInner::KnownHeader(k) => Self::Known(*k),
            HeaderNameInner::UnknownHeader(u) => Self::Unknown(u.reborrow().into_lower()),
        }
    }
}

impl From<KnownHeaderName> for QpackEntryName<'static> {
    fn from(value: KnownHeaderName) -> Self {
        Self::Known(value)
    }
}

impl From<PseudoHeaderName> for QpackEntryName<'static> {
    fn from(value: PseudoHeaderName) -> Self {
        Self::Pseudo(value)
    }
}

#[cfg(test)]
mod tests {
    use crate::{HeaderName, KnownHeaderName, headers::qpack::entry_name::QpackEntryName};

    #[test]
    fn khn_round_trip() {
        for khn in KnownHeaderName::VARIANTS {
            assert_eq!(
                QpackEntryName::try_from(khn.as_lower_str().as_bytes()).unwrap(),
                QpackEntryName::from(*khn)
            );
            assert_eq!(
                QpackEntryName::try_from(khn.as_lower_str().as_bytes().to_owned()).unwrap(),
                QpackEntryName::from(*khn)
            );
        }
    }

    #[test]
    fn pseudo_round_trip() {
        for khn in KnownHeaderName::VARIANTS {
            assert_eq!(
                QpackEntryName::try_from(khn.as_lower_str().as_bytes()).unwrap(),
                QpackEntryName::from(*khn)
            );
            assert_eq!(
                QpackEntryName::try_from(khn.as_lower_str().as_bytes().to_owned()).unwrap(),
                QpackEntryName::from(*khn)
            );
        }
    }

    #[test]
    fn other_round_trip() {
        assert_eq!(
            QpackEntryName::try_from(b"x-other".as_slice()).unwrap(),
            QpackEntryName::from(HeaderName::from("x-other"))
        );

        assert_eq!(
            QpackEntryName::try_from(b"x-other".to_vec()).unwrap(),
            QpackEntryName::from(HeaderName::from("x-other"))
        );
    }
}
