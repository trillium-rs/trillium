use super::STATIC_TABLE;
use crate::{
    KnownHeaderName as K,
    headers::entry_name::{EntryName, PseudoHeaderName as P},
};
use StaticLookup::{FullMatch, NameMatch, NoMatch};

/// Result of looking up a field line in the HPACK static table.
///
/// Indices are 1-based to match the wire encoding (RFC 7541 §2.3.1).
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

/// Map an [`EntryName`] to the (1-based) static-table indices that share that name, or
/// `None` if the name is not in the table. Compiled to a jump table over the `EntryName`
/// enum, so name-side lookup is O(1) — no linear scan over the 61-entry table.
pub(in crate::headers) const fn static_lookup_name(name: &EntryName) -> Option<&'static [u8]> {
    match name {
        EntryName::Pseudo(P::Authority) => Some(&[1]),
        EntryName::Pseudo(P::Method) => Some(&[2, 3]),
        EntryName::Pseudo(P::Path) => Some(&[4, 5]),
        EntryName::Pseudo(P::Scheme) => Some(&[6, 7]),
        EntryName::Pseudo(P::Status) => Some(&[8, 9, 10, 11, 12, 13, 14]),
        EntryName::Known(K::AcceptCharset) => Some(&[15]),
        EntryName::Known(K::AcceptEncoding) => Some(&[16]),
        EntryName::Known(K::AcceptLanguage) => Some(&[17]),
        EntryName::Known(K::AcceptRanges) => Some(&[18]),
        EntryName::Known(K::Accept) => Some(&[19]),
        EntryName::Known(K::AccessControlAllowOrigin) => Some(&[20]),
        EntryName::Known(K::Age) => Some(&[21]),
        EntryName::Known(K::Allow) => Some(&[22]),
        EntryName::Known(K::Authorization) => Some(&[23]),
        EntryName::Known(K::CacheControl) => Some(&[24]),
        EntryName::Known(K::ContentDisposition) => Some(&[25]),
        EntryName::Known(K::ContentEncoding) => Some(&[26]),
        EntryName::Known(K::ContentLanguage) => Some(&[27]),
        EntryName::Known(K::ContentLength) => Some(&[28]),
        EntryName::Known(K::ContentLocation) => Some(&[29]),
        EntryName::Known(K::ContentRange) => Some(&[30]),
        EntryName::Known(K::ContentType) => Some(&[31]),
        EntryName::Known(K::Cookie) => Some(&[32]),
        EntryName::Known(K::Date) => Some(&[33]),
        EntryName::Known(K::Etag) => Some(&[34]),
        EntryName::Known(K::Expect) => Some(&[35]),
        EntryName::Known(K::Expires) => Some(&[36]),
        EntryName::Known(K::From) => Some(&[37]),
        EntryName::Known(K::Host) => Some(&[38]),
        EntryName::Known(K::IfMatch) => Some(&[39]),
        EntryName::Known(K::IfModifiedSince) => Some(&[40]),
        EntryName::Known(K::IfNoneMatch) => Some(&[41]),
        EntryName::Known(K::IfRange) => Some(&[42]),
        EntryName::Known(K::IfUnmodifiedSince) => Some(&[43]),
        EntryName::Known(K::LastModified) => Some(&[44]),
        EntryName::Known(K::Link) => Some(&[45]),
        EntryName::Known(K::Location) => Some(&[46]),
        EntryName::Known(K::MaxForwards) => Some(&[47]),
        EntryName::Known(K::ProxyAuthenticate) => Some(&[48]),
        EntryName::Known(K::ProxyAuthorization) => Some(&[49]),
        EntryName::Known(K::Range) => Some(&[50]),
        EntryName::Known(K::Referer) => Some(&[51]),
        EntryName::Known(K::Refresh) => Some(&[52]),
        EntryName::Known(K::RetryAfter) => Some(&[53]),
        EntryName::Known(K::Server) => Some(&[54]),
        EntryName::Known(K::SetCookie) => Some(&[55]),
        EntryName::Known(K::StrictTransportSecurity) => Some(&[56]),
        EntryName::Known(K::TransferEncoding) => Some(&[57]),
        EntryName::Known(K::UserAgent) => Some(&[58]),
        EntryName::Known(K::Vary) => Some(&[59]),
        EntryName::Known(K::Via) => Some(&[60]),
        EntryName::Known(K::WwwAuthenticate) => Some(&[61]),
        _ => None,
    }
}

/// Look up a field name + value pair in the HPACK static table.
///
/// Walks only the candidate indices for the given name (typically 1; up to 7 for
/// `:status`), avoiding the 61-entry linear scan that the previous encoder did per
/// header line.
pub(in crate::headers) fn static_table_lookup(name: &EntryName, value: &[u8]) -> StaticLookup {
    let Some(indices) = static_lookup_name(name) else {
        return NoMatch;
    };

    for &i in indices {
        if STATIC_TABLE[(i - 1) as usize].1.as_bytes() == value {
            return FullMatch(i);
        }
    }

    NameMatch(indices[0])
}

#[cfg(test)]
mod tests {
    use super::{StaticLookup, static_table_lookup};
    use crate::headers::{
        entry_name::EntryName,
        hpack::static_table::{STATIC_TABLE, static_entry},
    };

    /// Every entry's (name, value) is recoverable via `FullMatch` at its 1-based index.
    #[test]
    fn lookup_matches_every_entry() {
        for (i, (name, value)) in STATIC_TABLE.iter().enumerate() {
            let wire_index = (i + 1) as u8;
            let header_name = EntryName::from(*name);
            let lookup = static_table_lookup(&header_name, value.as_bytes());

            // Names with shared entries (e.g. :status) only return the *first* matching
            // index for value `""`; otherwise we must hit FullMatch at this exact index.
            match lookup {
                StaticLookup::FullMatch(found) => {
                    let (found_name, found_value) = *static_entry(found as usize).unwrap();
                    assert_eq!(EntryName::from(found_name), header_name);
                    assert_eq!(found_value.as_bytes(), value.as_bytes());
                }
                _ => panic!(
                    "expected FullMatch for HPACK index {wire_index} ({}, {value:?}); got \
                     {lookup:?}",
                    name.as_str()
                ),
            }
        }
    }

    /// Name-only lookup (value not in table) yields a NameMatch pointing at *some* entry
    /// with that name, which matches the input name when looked up.
    #[test]
    fn name_match_for_unknown_value() {
        let header_name = EntryName::from(STATIC_TABLE[27].0); // index 28 = content-length
        let lookup = static_table_lookup(&header_name, b"99999");
        let StaticLookup::NameMatch(found) = lookup else {
            panic!("expected NameMatch, got {lookup:?}");
        };
        let (found_name, _) = *static_entry(found as usize).unwrap();
        assert_eq!(EntryName::from(found_name), header_name);
    }

    /// Names not in the static table fall through to NoMatch.
    #[test]
    fn no_match_for_unknown_name() {
        let header_name = EntryName::try_from(b"x-custom".as_slice()).unwrap();
        assert_eq!(
            static_table_lookup(&header_name, b"value"),
            StaticLookup::NoMatch
        );
    }
}
