use super::{PseudoHeaderName as P, STATIC_TABLE};
use crate::{KnownHeaderName as K, headers::qpack::entry_name::QpackEntryName};
use StaticLookup::{FullMatch, NameMatch, NoMatch};

/// Result of looking up a field line in the QPACK static table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, fieldwork::Fieldwork)]
#[allow(clippy::enum_variant_names)] // "Match" suffix is descriptive, not redundant
#[fieldwork(get)]
pub(in crate::headers) enum StaticLookup {
    /// Both name and value match a static table entry.
    FullMatch(#[field = "full_match"] u8),
    /// Name matches but value doesn't.
    NameMatch(#[field = "name_match"] u8),
    /// Name not in the static table.
    NoMatch,
}

pub(in crate::headers) const fn static_lookup_name(name: &QpackEntryName) -> Option<&'static [u8]> {
    match name {
        QpackEntryName::Pseudo(P::Authority) => Some(&[0]),
        QpackEntryName::Pseudo(P::Path) => Some(&[1]),
        QpackEntryName::Pseudo(P::Method) => Some(&[15, 16, 17, 18, 19, 20, 21]),
        QpackEntryName::Pseudo(P::Scheme) => Some(&[22, 23]),
        QpackEntryName::Pseudo(P::Status) => {
            Some(&[24, 25, 26, 27, 28, 63, 64, 65, 66, 67, 68, 69, 70, 71])
        }
        QpackEntryName::Known(K::Accept) => Some(&[29, 30]),
        QpackEntryName::Known(K::AcceptEncoding) => Some(&[31]),
        QpackEntryName::Known(K::AcceptLanguage) => Some(&[72]),
        QpackEntryName::Known(K::AcceptRanges) => Some(&[32]),
        QpackEntryName::Known(K::AccessControlAllowCredentials) => Some(&[73, 74]),
        QpackEntryName::Known(K::AccessControlAllowHeaders) => Some(&[33, 34, 75]),
        QpackEntryName::Known(K::AccessControlAllowMethods) => Some(&[76, 77, 78]),
        QpackEntryName::Known(K::AccessControlAllowOrigin) => Some(&[35]),
        QpackEntryName::Known(K::AccessControlExposeHeaders) => Some(&[79]),
        QpackEntryName::Known(K::AccessControlRequestHeaders) => Some(&[80]),
        QpackEntryName::Known(K::AccessControlRequestMethod) => Some(&[81, 82]),
        QpackEntryName::Known(K::Age) => Some(&[2]),
        QpackEntryName::Known(K::AltSvc) => Some(&[83]),
        QpackEntryName::Known(K::Authorization) => Some(&[84]),
        QpackEntryName::Known(K::CacheControl) => Some(&[36, 37, 38, 39, 40, 41]),
        QpackEntryName::Known(K::ContentDisposition) => Some(&[3]),
        QpackEntryName::Known(K::ContentEncoding) => Some(&[42, 43]),
        QpackEntryName::Known(K::ContentLength) => Some(&[4]),
        QpackEntryName::Known(K::ContentSecurityPolicy) => Some(&[85]),
        QpackEntryName::Known(K::ContentType) => {
            Some(&[44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54])
        }
        QpackEntryName::Known(K::Cookie) => Some(&[5]),
        QpackEntryName::Known(K::Date) => Some(&[6]),
        QpackEntryName::Known(K::EarlyData) => Some(&[86]),
        QpackEntryName::Known(K::Etag) => Some(&[7]),
        QpackEntryName::Known(K::ExpectCt) => Some(&[87]),
        QpackEntryName::Known(K::Forwarded) => Some(&[88]),
        QpackEntryName::Known(K::IfModifiedSince) => Some(&[8]),
        QpackEntryName::Known(K::IfNoneMatch) => Some(&[9]),
        QpackEntryName::Known(K::IfRange) => Some(&[89]),
        QpackEntryName::Known(K::LastModified) => Some(&[10]),
        QpackEntryName::Known(K::Link) => Some(&[11]),
        QpackEntryName::Known(K::Location) => Some(&[12]),
        QpackEntryName::Known(K::Origin) => Some(&[90]),
        QpackEntryName::Known(K::Purpose) => Some(&[91]),
        QpackEntryName::Known(K::Range) => Some(&[55]),
        QpackEntryName::Known(K::Referer) => Some(&[13]),
        QpackEntryName::Known(K::Server) => Some(&[92]),
        QpackEntryName::Known(K::SetCookie) => Some(&[14]),
        QpackEntryName::Known(K::StrictTransportSecurity) => Some(&[56, 57, 58]),
        QpackEntryName::Known(K::TimingAllowOrigin) => Some(&[93]),
        QpackEntryName::Known(K::UpgradeInsecureRequests) => Some(&[94]),
        QpackEntryName::Known(K::UserAgent) => Some(&[95]),
        QpackEntryName::Known(K::Vary) => Some(&[59, 60]),
        QpackEntryName::Known(K::XcontentTypeOptions) => Some(&[61]),
        QpackEntryName::Known(K::XforwardedFor) => Some(&[96]),
        QpackEntryName::Known(K::XframeOptions) => Some(&[97, 98]),
        QpackEntryName::Known(K::XxssProtection) => Some(&[62]),
        _ => None,
    }
}

pub(in crate::headers) const fn first_match(name: &QpackEntryName) -> Option<u8> {
    if let Some(indices) = static_lookup_name(name) {
        Some(indices[0])
    } else {
        None
    }
}

/// Look up a field name (regular header or pseudo-header) in the QPACK static table.
pub(in crate::headers) fn static_table_lookup(
    name: &QpackEntryName,
    value: Option<&[u8]>,
) -> StaticLookup {
    let Some(indices) = static_lookup_name(name) else {
        return NoMatch;
    };

    if let Some(value) = value {
        for &i in indices {
            if value == STATIC_TABLE[i as usize].1.as_bytes() {
                return FullMatch(i);
            }
        }
    }

    NameMatch(indices[0])
}

#[cfg(test)]
mod tests {
    use crate::headers::qpack::{
        entry_name::QpackEntryName,
        static_table::{STATIC_TABLE, StaticLookup, static_entry, static_table_lookup},
    };

    #[test]
    fn lookup_matches_every_entry() {
        for (index, (name, value)) in STATIC_TABLE.into_iter().enumerate() {
            let index = index as u8;
            let header_name = QpackEntryName::from(name);
            let lookup = static_table_lookup(&header_name, Some(value.as_bytes()));
            assert_eq!(lookup, StaticLookup::FullMatch(index));

            let lookup = static_table_lookup(&header_name, Some(b"other value".as_slice()));
            assert!(matches!(lookup, StaticLookup::NameMatch(_)));

            let matched_name = QpackEntryName::from(
                static_entry(lookup.name_match().unwrap() as usize)
                    .unwrap()
                    .0,
            );

            assert_eq!(matched_name, header_name);

            let lookup = static_table_lookup(&header_name, None);
            assert!(matches!(lookup, StaticLookup::NameMatch(_)));

            let matched_name = QpackEntryName::from(
                static_entry(lookup.name_match().unwrap() as usize)
                    .unwrap()
                    .0,
            );

            assert_eq!(matched_name, header_name);
        }

        assert_eq!(
            StaticLookup::NoMatch,
            static_table_lookup(
                &QpackEntryName::try_from(b"x-custom".as_slice()).unwrap(),
                Some(b"other".as_slice())
            )
        );
    }
}
