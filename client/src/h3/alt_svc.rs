use dashmap::DashMap;
use std::{
    ops::Deref,
    sync::Arc,
    time::{Duration, Instant},
};
use trillium_server_common::url::{Origin, Url};

/// How long to avoid a broken H3 endpoint before retrying. RFC 7838 leaves this unspecified;
/// five minutes matches common browser behaviour.
pub const DEFAULT_BROKEN_DURATION: Duration = Duration::from_secs(300);

/// Shared alt-svc cache, keyed by HTTP origin.
#[derive(Debug, Clone, Default)]
pub struct AltSvcCache(Arc<DashMap<Origin, AltSvcEntry>>);

impl Deref for AltSvcCache {
    type Target = DashMap<Origin, AltSvcEntry>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AltSvcCache {
    pub(crate) fn update(&self, alt_svc: &str, url: &Url) {
        let origin = url.origin();

        let origin_host = url.host_str().unwrap_or("");

        match parse_alt_svc_h3(alt_svc, origin_host) {
            None => {
                self.remove(&origin);
            }

            Some(mut entries) => {
                if let Some((host, port, max_age)) = entries.next() {
                    self.insert(origin, AltSvcEntry::new(host, port, max_age));
                }
            }
        }
    }
}

/// A cached HTTP/3 alternative endpoint advertised via `Alt-Svc`.
#[derive(Debug, Clone)]
pub struct AltSvcEntry {
    /// The host to connect to (may differ from the request origin).
    pub host: String,
    /// The UDP port to connect to.
    pub port: u16,
    /// When the advertisement expires (`ma` parameter from the server).
    expires: Instant,
    /// Set on connection failure; suppresses H3 attempts until elapsed.
    broken_until: Option<Instant>,
}

impl AltSvcEntry {
    /// Create a new entry that expires after `max_age`.
    pub fn new(host: String, port: u16, max_age: Duration) -> Self {
        Self {
            host,
            port,
            expires: Instant::now() + max_age,
            broken_until: None,
        }
    }

    /// Returns `true` if this entry should be used for an H3 request right now.
    pub fn is_usable(&self) -> bool {
        let now = Instant::now();
        now < self.expires && self.broken_until.map_or(true, |t| now >= t)
    }

    /// Mark this endpoint as temporarily unavailable for `duration`.
    pub fn mark_broken(&mut self, duration: Duration) {
        self.broken_until = Some(Instant::now() + duration);
    }
}

/// Parse an `Alt-Svc` header value, yielding `(host, port, max_age)` for each `h3` entry.
///
/// Returns `None` if the value is `clear`, signalling that all alternatives should be removed.
/// Entries for other ALPNs (h2, etc.) are silently skipped.
pub fn parse_alt_svc_h3<'a>(
    value: &'a str,
    origin_host: &'a str,
) -> Option<impl Iterator<Item = (String, u16, Duration)> + 'a> {
    if value.trim().eq_ignore_ascii_case("clear") {
        return None;
    }

    Some(value.split(',').filter_map(move |entry| {
        let entry = entry.trim();
        let (alpn, rest) = entry.split_once('=')?;
        if !alpn.trim().eq_ignore_ascii_case("h3") {
            return None;
        }
        // alt-authority is a quoted string: "[host]:port"
        let rest = rest.trim().strip_prefix('"')?;
        let (alt_authority, params) = rest.split_once('"')?;
        // rsplit on ':' handles IPv6 addresses like [::1]:443
        let (host, port_str) = alt_authority.rsplit_once(':')?;
        let port = port_str.parse::<u16>().ok()?;
        let host = if host.is_empty() {
            origin_host.to_string()
        } else {
            host.to_string()
        };
        let max_age = parse_max_age(params).unwrap_or(Duration::from_secs(86400));
        Some((host, port, max_age))
    }))
}

fn parse_max_age(params: &str) -> Option<Duration> {
    for param in params.split(';') {
        if let Some(val) = param.trim().strip_prefix("ma=") {
            if let Ok(secs) = val.trim().parse::<u64>() {
                return Some(Duration::from_secs(secs));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_same_host() {
        let entries: Vec<_> = parse_alt_svc_h3(r#"h3=":443"; ma=86400"#, "example.com")
            .unwrap()
            .collect();
        assert_eq!(
            entries,
            [("example.com".into(), 443, Duration::from_secs(86400))]
        );
    }

    #[test]
    fn parse_different_host() {
        let entries: Vec<_> = parse_alt_svc_h3(r#"h3="alt.example.com:8443""#, "example.com")
            .unwrap()
            .collect();
        assert_eq!(
            entries,
            [("alt.example.com".into(), 8443, Duration::from_secs(86400))]
        );
    }

    #[test]
    fn skip_other_alpns() {
        let entries: Vec<_> = parse_alt_svc_h3(r#"h2=":443", h3=":443"; ma=3600"#, "example.com")
            .unwrap()
            .collect();
        assert_eq!(
            entries,
            [("example.com".into(), 443, Duration::from_secs(3600))]
        );
    }

    #[test]
    fn clear_returns_none() {
        assert!(parse_alt_svc_h3("clear", "example.com").is_none());
        assert!(parse_alt_svc_h3("  CLEAR  ", "example.com").is_none());
    }

    #[test]
    fn multiple_h3_entries_yields_all() {
        let entries: Vec<_> =
            parse_alt_svc_h3(r#"h3=":443"; ma=3600, h3=":8443"; ma=600"#, "example.com")
                .unwrap()
                .collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].1, 443);
        assert_eq!(entries[1].1, 8443);
    }
}
