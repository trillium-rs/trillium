use std::time::SystemTime;

/// A parsed `Range` request, before resolution against a file's size.
///
/// Multi-range requests (e.g. `bytes=0-100,200-300`) are not represented —
/// [`parse`] returns `None` for them so the handler can fall through to a
/// 200 full-body response. This matches the common pattern in nginx and
/// other static-file servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RangeSpec {
    /// `bytes=START-END` (inclusive).
    FromTo(u64, u64),
    /// `bytes=START-` — from `START` to the end of the file.
    From(u64),
    /// `bytes=-N` — the last `N` bytes of the file.
    Suffix(u64),
}

/// Parse the value of a `Range` request header. Returns `None` for any of:
/// missing/wrong unit, unparseable, multi-range, or inverted single range.
pub(crate) fn parse(header: &str) -> Option<RangeSpec> {
    let bytes_part = header.strip_prefix("bytes=")?.trim();
    if bytes_part.contains(',') {
        return None;
    }
    let (start, end) = bytes_part.split_once('-')?;
    match (start.trim(), end.trim()) {
        ("", "") => None,
        ("", suffix) => suffix.parse().ok().map(RangeSpec::Suffix),
        (start, "") => start.parse().ok().map(RangeSpec::From),
        (start, end) => {
            let s: u64 = start.parse().ok()?;
            let e: u64 = end.parse().ok()?;
            (s <= e).then_some(RangeSpec::FromTo(s, e))
        }
    }
}

/// Resolve a parsed range against the total content size. Returns the
/// inclusive `(start, end)` byte indices, or `None` if the range is not
/// satisfiable (the caller should respond 416 in that case).
pub(crate) fn resolve(spec: RangeSpec, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    match spec {
        RangeSpec::FromTo(s, e) => (s < total).then(|| (s, e.min(total - 1))),
        RangeSpec::From(s) => (s < total).then_some((s, total - 1)),
        RangeSpec::Suffix(n) => {
            if n == 0 {
                None
            } else {
                let n = n.min(total);
                Some((total - n, total - 1))
            }
        }
    }
}

/// Returns true if an `If-Range` header value matches the resource's
/// current validators per RFC 9110 14.1.2 (strong comparison only).
///
/// `If-Range` carries either an entity-tag or an HTTP-date. If it doesn't
/// match, the handler must serve the full body with status 200 instead of
/// honoring the `Range`.
pub(crate) fn if_range_matches(
    if_range: &str,
    etag: Option<&str>,
    last_modified: Option<SystemTime>,
) -> bool {
    let trimmed = if_range.trim();

    // Per RFC 9110 14.1.2, comparison is strong-only. Weak entity-tags
    // (those starting with `W/`) never match — a conformant client should
    // not send one in If-Range, but reject them defensively.
    if trimmed.starts_with("W/") {
        return false;
    }

    if let Some(et) = etag
        && trimmed == et
    {
        return true;
    }

    // HTTP-date carries second precision; truncate the resource's mtime to
    // the same granularity for the comparison.
    if let Some(modified) = last_modified
        && let Ok(date) = httpdate::parse_http_date(trimmed)
        && let (Ok(m), Ok(d)) = (
            modified.duration_since(SystemTime::UNIX_EPOCH),
            date.duration_since(SystemTime::UNIX_EPOCH),
        )
        && m.as_secs() == d.as_secs()
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_forms() {
        assert_eq!(parse("bytes=0-499"), Some(RangeSpec::FromTo(0, 499)));
        assert_eq!(parse("bytes=500-"), Some(RangeSpec::From(500)));
        assert_eq!(parse("bytes=-50"), Some(RangeSpec::Suffix(50)));
    }

    #[test]
    fn parse_rejects() {
        assert_eq!(parse("bytes=0-100,200-300"), None);
        assert_eq!(parse("seconds=0-10"), None);
        assert_eq!(parse("bytes=500-100"), None);
        assert_eq!(parse("bytes=abc-def"), None);
        assert_eq!(parse("bytes=-"), None);
    }

    #[test]
    fn resolve_basics() {
        assert_eq!(resolve(RangeSpec::FromTo(0, 99), 1000), Some((0, 99)));
        assert_eq!(resolve(RangeSpec::FromTo(0, 99999), 1000), Some((0, 999)));
        assert_eq!(resolve(RangeSpec::FromTo(1000, 2000), 1000), None);
        assert_eq!(resolve(RangeSpec::From(500), 1000), Some((500, 999)));
        assert_eq!(resolve(RangeSpec::From(1000), 1000), None);
        assert_eq!(resolve(RangeSpec::Suffix(100), 1000), Some((900, 999)));
        assert_eq!(resolve(RangeSpec::Suffix(2000), 1000), Some((0, 999)));
        assert_eq!(resolve(RangeSpec::Suffix(0), 1000), None);
    }

    #[test]
    fn if_range_etag() {
        assert!(if_range_matches("\"abc\"", Some("\"abc\""), None));
        assert!(!if_range_matches("\"xyz\"", Some("\"abc\""), None));
        // weak etags never match
        assert!(!if_range_matches("W/\"abc\"", Some("W/\"abc\""), None));
    }

    #[test]
    fn if_range_date() {
        let modified = httpdate::parse_http_date("Wed, 21 Oct 2026 07:28:00 GMT").unwrap();
        assert!(if_range_matches(
            "Wed, 21 Oct 2026 07:28:00 GMT",
            None,
            Some(modified)
        ));
        assert!(!if_range_matches(
            "Wed, 21 Oct 2026 07:28:01 GMT",
            None,
            Some(modified)
        ));
    }
}
