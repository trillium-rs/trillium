use super::STATIC_TABLE;
use crate::{HeaderName, HeaderValue, KnownHeaderName as K};
use StaticLookup::{FullMatch, NameMatch, NoMatch};

/// Result of looking up a field line in the QPACK static table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StaticLookup {
    /// Both name and value match a static table entry.
    FullMatch(u8),
    /// Name matches but value doesn't.
    NameMatch(u8),
    /// Name not in the static table.
    NoMatch,
}

/// Look up a regular (non-pseudo) header in the QPACK static table.
pub(crate) fn static_table_lookup(name: &HeaderName<'_>, value: &HeaderValue) -> StaticLookup {
    let indices: &[u8] = match name.as_known() {
        Some(K::Accept) => &[29, 30],
        Some(K::AcceptEncoding) => &[31],
        Some(K::AcceptLanguage) => &[72],
        Some(K::AcceptRanges) => &[32],
        Some(K::AccessControlAllowCredentials) => &[73, 74],
        Some(K::AccessControlAllowHeaders) => &[33, 34, 75],
        Some(K::AccessControlAllowMethods) => &[76, 77, 78],
        Some(K::AccessControlAllowOrigin) => &[35],
        Some(K::AccessControlExposeHeaders) => &[79],
        Some(K::AccessControlRequestHeaders) => &[80],
        Some(K::AccessControlRequestMethod) => &[81, 82],
        Some(K::Age) => &[2],
        Some(K::AltSvc) => &[83],
        Some(K::Authorization) => &[84],
        Some(K::CacheControl) => &[36, 37, 38, 39, 40, 41],
        Some(K::ContentDisposition) => &[3],
        Some(K::ContentEncoding) => &[42, 43],
        Some(K::ContentLength) => &[4],
        Some(K::ContentSecurityPolicy) => &[85],
        Some(K::ContentType) => &[44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54],
        Some(K::Cookie) => &[5],
        Some(K::Date) => &[6],
        Some(K::EarlyData) => &[86],
        Some(K::Etag) => &[7],
        Some(K::ExpectCt) => &[87],
        Some(K::Forwarded) => &[88],
        Some(K::IfModifiedSince) => &[8],
        Some(K::IfNoneMatch) => &[9],
        Some(K::IfRange) => &[89],
        Some(K::LastModified) => &[10],
        Some(K::Link) => &[11],
        Some(K::Location) => &[12],
        Some(K::Origin) => &[90],
        Some(K::Purpose) => &[91],
        Some(K::Range) => &[55],
        Some(K::Referer) => &[13],
        Some(K::Server) => &[92],
        Some(K::SetCookie) => &[14],
        Some(K::StrictTransportSecurity) => &[56, 57, 58],
        Some(K::TimingAllowOrigin) => &[93],
        Some(K::UpgradeInsecureRequests) => &[94],
        Some(K::UserAgent) => &[95],
        Some(K::Vary) => &[59, 60],
        Some(K::XcontentTypeOptions) => &[61],
        Some(K::XforwardedFor) => &[96],
        Some(K::XframeOptions) => &[97, 98],
        Some(K::XxssProtection) => &[62],
        _ => return NoMatch,
    };

    for &i in indices {
        if value == STATIC_TABLE[i as usize].1 {
            return FullMatch(i);
        }
    }
    NameMatch(indices[0])
}

#[cfg(test)]
mod tests {
    use super::{
        super::{STATIC_TABLE, StaticHeaderName},
        *,
    };

    #[test]
    fn lookup_matches_every_header_entry() {
        for (index, (name, value)) in STATIC_TABLE.iter().enumerate() {
            let index = index as u8;
            let StaticHeaderName::Header(known) = name else {
                continue; // skip pseudo-headers — tested via encoder round-trips
            };
            let header_name = HeaderName::from(*known);
            let header_value = HeaderValue::from(*value);
            let lookup = static_table_lookup(&header_name, &header_value);

            if value.is_empty() {
                match lookup {
                    NameMatch(i) => assert_eq!(
                        STATIC_TABLE[i as usize].0, *name,
                        "index {index} ({name}): NameMatch({i}) points to wrong name",
                    ),
                    FullMatch(_) => {}
                    NoMatch => panic!("index {index} ({name}): expected NameMatch, got NoMatch"),
                }
            } else {
                assert!(
                    matches!(lookup, FullMatch(i) if i == index),
                    "index {index} ({name}:{value}): expected FullMatch({index}), got {lookup:?}",
                );
            }
        }
    }
}
