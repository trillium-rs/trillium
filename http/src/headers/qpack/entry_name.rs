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
use crate::HeaderName;

/// A QPACK dynamic table entry's name — either a regular header name or a pseudo-header.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub(crate) enum QpackEntryName {
    /// A regular header name (known or unknown to trillium).
    Header(HeaderName<'static>),
    /// An HTTP pseudo-header name (e.g. `:method`, `:path`).
    Pseudo(PseudoHeaderName),
}

impl QpackEntryName {
    /// The bytes of the name as they would appear on the wire. For pseudo-headers this
    /// includes the leading `:`.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Header(h) => h.as_ref().as_bytes(),
            Self::Pseudo(p) => p.as_str().as_bytes(),
        }
    }

    /// Length in bytes of the name as it appears on the wire. Used for entry-size
    /// calculation (RFC 9204 §3.2.1).
    pub(crate) fn len(&self) -> usize {
        self.as_bytes().len()
    }
}

impl From<StaticHeaderName> for QpackEntryName {
    fn from(s: StaticHeaderName) -> Self {
        match s {
            StaticHeaderName::Header(known) => Self::Header(HeaderName::from(known)),
            StaticHeaderName::Pseudo(pseudo) => Self::Pseudo(pseudo),
        }
    }
}

impl From<HeaderName<'static>> for QpackEntryName {
    fn from(h: HeaderName<'static>) -> Self {
        Self::Header(h)
    }
}
