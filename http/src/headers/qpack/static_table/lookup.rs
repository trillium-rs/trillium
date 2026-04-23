use super::STATIC_TABLE;
use crate::{
    KnownHeaderName as K,
    headers::entry_name::{EntryName, PseudoHeaderName as P},
};
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

pub(in crate::headers) const fn static_lookup_name(name: &EntryName) -> Option<&'static [u8]> {
    match name {
        EntryName::Pseudo(P::Authority) => Some(&[0]),
        EntryName::Pseudo(P::Path) => Some(&[1]),
        EntryName::Pseudo(P::Method) => Some(&[15, 16, 17, 18, 19, 20, 21]),
        EntryName::Pseudo(P::Scheme) => Some(&[22, 23]),
        EntryName::Pseudo(P::Status) => {
            Some(&[24, 25, 26, 27, 28, 63, 64, 65, 66, 67, 68, 69, 70, 71])
        }
        EntryName::Known(K::Accept) => Some(&[29, 30]),
        EntryName::Known(K::AcceptEncoding) => Some(&[31]),
        EntryName::Known(K::AcceptLanguage) => Some(&[72]),
        EntryName::Known(K::AcceptRanges) => Some(&[32]),
        EntryName::Known(K::AccessControlAllowCredentials) => Some(&[73, 74]),
        EntryName::Known(K::AccessControlAllowHeaders) => Some(&[33, 34, 75]),
        EntryName::Known(K::AccessControlAllowMethods) => Some(&[76, 77, 78]),
        EntryName::Known(K::AccessControlAllowOrigin) => Some(&[35]),
        EntryName::Known(K::AccessControlExposeHeaders) => Some(&[79]),
        EntryName::Known(K::AccessControlRequestHeaders) => Some(&[80]),
        EntryName::Known(K::AccessControlRequestMethod) => Some(&[81, 82]),
        EntryName::Known(K::Age) => Some(&[2]),
        EntryName::Known(K::AltSvc) => Some(&[83]),
        EntryName::Known(K::Authorization) => Some(&[84]),
        EntryName::Known(K::CacheControl) => Some(&[36, 37, 38, 39, 40, 41]),
        EntryName::Known(K::ContentDisposition) => Some(&[3]),
        EntryName::Known(K::ContentEncoding) => Some(&[42, 43]),
        EntryName::Known(K::ContentLength) => Some(&[4]),
        EntryName::Known(K::ContentSecurityPolicy) => Some(&[85]),
        EntryName::Known(K::ContentType) => Some(&[44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54]),
        EntryName::Known(K::Cookie) => Some(&[5]),
        EntryName::Known(K::Date) => Some(&[6]),
        EntryName::Known(K::EarlyData) => Some(&[86]),
        EntryName::Known(K::Etag) => Some(&[7]),
        EntryName::Known(K::ExpectCt) => Some(&[87]),
        EntryName::Known(K::Forwarded) => Some(&[88]),
        EntryName::Known(K::IfModifiedSince) => Some(&[8]),
        EntryName::Known(K::IfNoneMatch) => Some(&[9]),
        EntryName::Known(K::IfRange) => Some(&[89]),
        EntryName::Known(K::LastModified) => Some(&[10]),
        EntryName::Known(K::Link) => Some(&[11]),
        EntryName::Known(K::Location) => Some(&[12]),
        EntryName::Known(K::Origin) => Some(&[90]),
        EntryName::Known(K::Purpose) => Some(&[91]),
        EntryName::Known(K::Range) => Some(&[55]),
        EntryName::Known(K::Referer) => Some(&[13]),
        EntryName::Known(K::Server) => Some(&[92]),
        EntryName::Known(K::SetCookie) => Some(&[14]),
        EntryName::Known(K::StrictTransportSecurity) => Some(&[56, 57, 58]),
        EntryName::Known(K::TimingAllowOrigin) => Some(&[93]),
        EntryName::Known(K::UpgradeInsecureRequests) => Some(&[94]),
        EntryName::Known(K::UserAgent) => Some(&[95]),
        EntryName::Known(K::Vary) => Some(&[59, 60]),
        EntryName::Known(K::XcontentTypeOptions) => Some(&[61]),
        EntryName::Known(K::XforwardedFor) => Some(&[96]),
        EntryName::Known(K::XframeOptions) => Some(&[97, 98]),
        EntryName::Known(K::XxssProtection) => Some(&[62]),
        _ => None,
    }
}

pub(in crate::headers) const fn first_match(name: &EntryName) -> Option<u8> {
    if let Some(indices) = static_lookup_name(name) {
        Some(indices[0])
    } else {
        None
    }
}

/// Look up a field name (regular header or pseudo-header) in the QPACK static table.
pub(in crate::headers) fn static_table_lookup(
    name: &EntryName,
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
    use crate::headers::{
        entry_name::EntryName,
        qpack::static_table::{STATIC_TABLE, StaticLookup, static_entry, static_table_lookup},
    };

    #[test]
    fn lookup_matches_every_entry() {
        for (index, (name, value)) in STATIC_TABLE.into_iter().enumerate() {
            let index = index as u8;
            let header_name = EntryName::from(name);
            let lookup = static_table_lookup(&header_name, Some(value.as_bytes()));
            assert_eq!(lookup, StaticLookup::FullMatch(index));

            let lookup = static_table_lookup(&header_name, Some(b"other value".as_slice()));
            assert!(matches!(lookup, StaticLookup::NameMatch(_)));

            let matched_name = EntryName::from(
                static_entry(lookup.name_match().unwrap() as usize)
                    .unwrap()
                    .0,
            );

            assert_eq!(matched_name, header_name);

            let lookup = static_table_lookup(&header_name, None);
            assert!(matches!(lookup, StaticLookup::NameMatch(_)));

            let matched_name = EntryName::from(
                static_entry(lookup.name_match().unwrap() as usize)
                    .unwrap()
                    .0,
            );

            assert_eq!(matched_name, header_name);
        }

        assert_eq!(
            StaticLookup::NoMatch,
            static_table_lookup(
                &EntryName::try_from(b"x-custom".as_slice()).unwrap(),
                Some(b"other".as_slice())
            )
        );
    }
}
