//! RFC 7541 Appendix A static table (61 entries, 1-indexed).

// Bring every KnownHeaderName variant the table uses into scope with the `K::` alias so
// `KnownHeaderName::From` (the HTTP `From` header) doesn't shadow `std::convert::From`.
use crate::{
    KnownHeaderName as K,
    headers::{
        compression_error::CompressionError,
        entry_name::{EntryName, PseudoHeaderName},
    },
};
use PseudoHeaderName::{Authority, Method, Path, Scheme, Status};
use StaticHeaderName::{Header, Pseudo};
use std::fmt::{self, Display, Formatter};

mod lookup;
pub(in crate::headers) use lookup::static_table_lookup;

/// A name in the HPACK static table — either a regular header or a pseudo-header.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(in crate::headers) enum StaticHeaderName {
    Header(K),
    Pseudo(PseudoHeaderName),
}

impl StaticHeaderName {
    /// The canonical wire-form string for this name.
    pub(in crate::headers) fn as_str(self) -> &'static str {
        match self {
            Header(k) => k.as_lower_str(),
            Pseudo(p) => p.as_str(),
        }
    }
}

impl AsRef<str> for StaticHeaderName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Display for StaticHeaderName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl From<StaticHeaderName> for EntryName<'static> {
    fn from(s: StaticHeaderName) -> Self {
        match s {
            Header(known) => known.into(),
            Pseudo(pseudo) => pseudo.into(),
        }
    }
}

/// Look up entry at the given 1-based index (1..=61).
///
/// # Errors
///
/// Returns [`CompressionError::InvalidStaticIndex`] if `index` is 0 or greater than 61.
pub(in crate::headers) fn static_entry(
    index: usize,
) -> Result<&'static (StaticHeaderName, &'static str), CompressionError> {
    if index == 0 {
        return Err(CompressionError::InvalidStaticIndex(index));
    }
    STATIC_TABLE
        .get(index - 1)
        .ok_or(CompressionError::InvalidStaticIndex(index))
}

/// Physically 0-indexed; callers add 1 via [`static_entry`].
pub(super) const STATIC_TABLE: [(StaticHeaderName, &str); 61] = [
    (Pseudo(Authority), ""),
    (Pseudo(Method), "GET"),
    (Pseudo(Method), "POST"),
    (Pseudo(Path), "/"),
    (Pseudo(Path), "/index.html"),
    (Pseudo(Scheme), "http"),
    (Pseudo(Scheme), "https"),
    (Pseudo(Status), "200"),
    (Pseudo(Status), "204"),
    (Pseudo(Status), "206"),
    (Pseudo(Status), "304"),
    (Pseudo(Status), "400"),
    (Pseudo(Status), "404"),
    (Pseudo(Status), "500"),
    (Header(K::AcceptCharset), ""),
    (Header(K::AcceptEncoding), "gzip, deflate"),
    (Header(K::AcceptLanguage), ""),
    (Header(K::AcceptRanges), ""),
    (Header(K::Accept), ""),
    (Header(K::AccessControlAllowOrigin), ""),
    (Header(K::Age), ""),
    (Header(K::Allow), ""),
    (Header(K::Authorization), ""),
    (Header(K::CacheControl), ""),
    (Header(K::ContentDisposition), ""),
    (Header(K::ContentEncoding), ""),
    (Header(K::ContentLanguage), ""),
    (Header(K::ContentLength), ""),
    (Header(K::ContentLocation), ""),
    (Header(K::ContentRange), ""),
    (Header(K::ContentType), ""),
    (Header(K::Cookie), ""),
    (Header(K::Date), ""),
    (Header(K::Etag), ""),
    (Header(K::Expect), ""),
    (Header(K::Expires), ""),
    (Header(K::From), ""),
    (Header(K::Host), ""),
    (Header(K::IfMatch), ""),
    (Header(K::IfModifiedSince), ""),
    (Header(K::IfNoneMatch), ""),
    (Header(K::IfRange), ""),
    (Header(K::IfUnmodifiedSince), ""),
    (Header(K::LastModified), ""),
    (Header(K::Link), ""),
    (Header(K::Location), ""),
    (Header(K::MaxForwards), ""),
    (Header(K::ProxyAuthenticate), ""),
    (Header(K::ProxyAuthorization), ""),
    (Header(K::Range), ""),
    (Header(K::Referer), ""),
    (Header(K::Refresh), ""),
    (Header(K::RetryAfter), ""),
    (Header(K::Server), ""),
    (Header(K::SetCookie), ""),
    (Header(K::StrictTransportSecurity), ""),
    (Header(K::TransferEncoding), ""),
    (Header(K::UserAgent), ""),
    (Header(K::Vary), ""),
    (Header(K::Via), ""),
    (Header(K::WwwAuthenticate), ""),
];

#[cfg(test)]
mod tests {
    use super::{STATIC_TABLE, StaticHeaderName, static_entry};
    use crate::headers::{compression_error::CompressionError, entry_name::PseudoHeaderName};

    #[test]
    fn table_has_61_entries() {
        assert_eq!(STATIC_TABLE.len(), 61);
    }

    #[test]
    fn one_based_indexing() {
        // Index 1 → (:authority, "")
        let (name, value) = *static_entry(1).unwrap();
        assert!(matches!(
            name,
            StaticHeaderName::Pseudo(PseudoHeaderName::Authority)
        ));
        assert_eq!(value, "");

        // Index 61 → (www-authenticate, "")
        let (name, value) = *static_entry(61).unwrap();
        assert_eq!(name.as_str(), "www-authenticate");
        assert_eq!(value, "");
    }

    #[test]
    fn index_zero_is_invalid() {
        assert!(matches!(
            static_entry(0),
            Err(CompressionError::InvalidStaticIndex(0))
        ));
    }

    #[test]
    fn index_above_62_is_invalid() {
        assert!(matches!(
            static_entry(62),
            Err(CompressionError::InvalidStaticIndex(62))
        ));
        assert!(matches!(
            static_entry(1000),
            Err(CompressionError::InvalidStaticIndex(1000))
        ));
    }

    /// Spot-check a few RFC 7541 Appendix A entries for sanity.
    #[test]
    fn known_entries() {
        // GET — index 2
        let (_, v) = static_entry(2).unwrap();
        assert_eq!(*v, "GET");

        // :status 200 — index 8
        let (_, v) = static_entry(8).unwrap();
        assert_eq!(*v, "200");

        // accept-encoding: gzip, deflate — index 16
        let (name, v) = static_entry(16).unwrap();
        assert_eq!(name.as_str(), "accept-encoding");
        assert_eq!(*v, "gzip, deflate");

        // set-cookie — index 55
        let (name, v) = static_entry(55).unwrap();
        assert_eq!(name.as_str(), "set-cookie");
        assert_eq!(*v, "");
    }

    /// Every entry's name should be ascii-lowercase.
    #[test]
    fn all_names_lowercase() {
        for (i, (name, _)) in STATIC_TABLE.iter().enumerate() {
            let s = name.as_str();
            assert!(
                s.bytes().all(|b| !b.is_ascii_uppercase()),
                "entry {} ({s:?}) has uppercase bytes",
                i + 1
            );
        }
    }

    /// Full cross-check against RFC 7541 Appendix A Table 1. Independent hand-typed copy of
    /// the wire strings — any drift between this list and the [`STATIC_TABLE`] array fires
    /// on index mismatch.
    #[test]
    fn matches_rfc_7541_appendix_a() {
        const EXPECTED: [(&str, &str); 61] = [
            (":authority", ""),
            (":method", "GET"),
            (":method", "POST"),
            (":path", "/"),
            (":path", "/index.html"),
            (":scheme", "http"),
            (":scheme", "https"),
            (":status", "200"),
            (":status", "204"),
            (":status", "206"),
            (":status", "304"),
            (":status", "400"),
            (":status", "404"),
            (":status", "500"),
            ("accept-charset", ""),
            ("accept-encoding", "gzip, deflate"),
            ("accept-language", ""),
            ("accept-ranges", ""),
            ("accept", ""),
            ("access-control-allow-origin", ""),
            ("age", ""),
            ("allow", ""),
            ("authorization", ""),
            ("cache-control", ""),
            ("content-disposition", ""),
            ("content-encoding", ""),
            ("content-language", ""),
            ("content-length", ""),
            ("content-location", ""),
            ("content-range", ""),
            ("content-type", ""),
            ("cookie", ""),
            ("date", ""),
            ("etag", ""),
            ("expect", ""),
            ("expires", ""),
            ("from", ""),
            ("host", ""),
            ("if-match", ""),
            ("if-modified-since", ""),
            ("if-none-match", ""),
            ("if-range", ""),
            ("if-unmodified-since", ""),
            ("last-modified", ""),
            ("link", ""),
            ("location", ""),
            ("max-forwards", ""),
            ("proxy-authenticate", ""),
            ("proxy-authorization", ""),
            ("range", ""),
            ("referer", ""),
            ("refresh", ""),
            ("retry-after", ""),
            ("server", ""),
            ("set-cookie", ""),
            ("strict-transport-security", ""),
            ("transfer-encoding", ""),
            ("user-agent", ""),
            ("vary", ""),
            ("via", ""),
            ("www-authenticate", ""),
        ];

        for (i, (expected_name, expected_value)) in EXPECTED.iter().enumerate() {
            let index = i + 1;
            let (name, value) = *static_entry(index).expect("entry exists");
            assert_eq!(
                name.as_str(),
                *expected_name,
                "name mismatch at HPACK index {index}"
            );
            assert_eq!(
                value, *expected_value,
                "value mismatch at HPACK index {index}"
            );
        }
    }
}
