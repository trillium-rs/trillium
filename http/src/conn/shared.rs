use crate::{Headers, KnownHeaderName, Method, headers::hpack::FieldSection};
use std::borrow::Cow;

/// Whether a request-target authority (absolute-form authority, or the `:authority`
/// pseudo-header) is equivalent to a `Host` header value under scheme-based normalization:
/// hosts compare case-insensitively, userinfo is excluded, and a missing port is equivalent
/// to the scheme's default port (`80` for http/ws, `443` for https/wss). A non-default port
/// such as `:8080` is significant and only matches the same explicit port.
pub(super) fn authority_matches_host(authority: &str, host: &str, scheme: Option<&str>) -> bool {
    let (authority_host, authority_port) = split_host_port(authority);
    let (host_host, host_port) = split_host_port(host);

    if !authority_host.eq_ignore_ascii_case(host_host) {
        return false;
    }

    let default_port = match scheme {
        Some("http" | "ws") => Some("80"),
        Some("https" | "wss") => Some("443"),
        _ => None,
    };

    authority_port.or(default_port) == host_port.or(default_port)
}

/// Split an `[userinfo@]host[:port]` authority into `(host, port)`, dropping any userinfo and
/// handling bracketed IPv6 literals (`[::1]:8080`).
fn split_host_port(authority: &str) -> (&str, Option<&str>) {
    let authority = authority.split_once('@').map_or(authority, |(_, h)| h);

    match authority.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((host, after)) => (host, after.strip_prefix(':')),
            None => (authority, None),
        },
        None => match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::authority_matches_host;

    #[test]
    fn authority_host_equivalence() {
        // exact match
        assert!(authority_matches_host(
            "example.com:8080",
            "example.com:8080",
            Some("http")
        ));
        // default-port normalization
        assert!(authority_matches_host(
            "example.com:443",
            "example.com",
            Some("https")
        ));
        assert!(authority_matches_host(
            "example.com",
            "example.com:443",
            Some("https")
        ));
        assert!(authority_matches_host(
            "example.com:80",
            "example.com",
            Some("http")
        ));
        // case-insensitive host
        assert!(authority_matches_host(
            "Example.COM",
            "example.com",
            Some("http")
        ));
        // userinfo excluded
        assert!(authority_matches_host(
            "user@example.com",
            "example.com",
            Some("http")
        ));
        // IPv6 literals
        assert!(authority_matches_host(
            "[::1]:8080",
            "[::1]:8080",
            Some("http")
        ));
        assert!(authority_matches_host("[::1]", "[::1]:443", Some("https")));

        // a non-default port is significant
        assert!(!authority_matches_host(
            "example.com",
            "example.com:8080",
            Some("http")
        ));
        assert!(!authority_matches_host(
            "example.com:8080",
            "example.com:8081",
            Some("http")
        ));
        // different hosts never match
        assert!(!authority_matches_host(
            "example.com",
            "evil.example.com",
            Some("http")
        ));
        // without a known scheme there is no default port to normalize against
        assert!(!authority_matches_host(
            "example.com:443",
            "example.com",
            None
        ));
    }
}

/// Header names whose semantics only apply at the HTTP/1 layer.
///
/// HTTP/2 and HTTP/3 call these "connection-specific" and forbid them in requests and
/// responses.
pub(super) const H1_ONLY_HEADERS: [KnownHeaderName; 5] = [
    KnownHeaderName::Connection,
    KnownHeaderName::KeepAlive,
    KnownHeaderName::ProxyConnection,
    KnownHeaderName::TransferEncoding,
    KnownHeaderName::Upgrade,
];

/// Validated request pseudo-headers + headers, the common output of
/// [`validate_h2h3_request`].
pub(crate) struct ValidatedRequest {
    pub method: Method,
    pub path: Cow<'static, str>,
    pub authority: Option<Cow<'static, str>>,
    pub scheme: Option<Cow<'static, str>>,
    pub protocol: Option<Cow<'static, str>>,
    pub request_headers: Headers,
}

impl ValidatedRequest {
    /// Shared HTTP/2 + HTTP/3 request-validation per RFC 9113 and RFC 9114.
    ///
    /// Both protocols apply the same malformed-message rules to incoming requests:
    /// no `:status` pseudo, required `:method`, non-empty `:path` (or CONNECT default),
    /// `:scheme` required for non-CONNECT, `:authority` required for CONNECT, `:authority`
    /// or `Host` required when `:scheme` is `http`/`https`, no `Host`/`:authority`
    /// mismatch, no [`H1_ONLY_HEADERS`], and `TE` restricted to `trailers`. Returns `None`
    /// on any violation; the caller maps to its protocol-specific error code.
    pub(super) fn new(mut field_section: FieldSection<'static>) -> Option<ValidatedRequest> {
        let pseudo_headers = field_section.pseudo_headers_mut();

        // `:status` is response-only; reject it on requests.
        if pseudo_headers.status().is_some() {
            return None;
        }

        let method = pseudo_headers.take_method();
        let path = pseudo_headers.take_path();
        let authority = pseudo_headers.take_authority();
        let scheme = pseudo_headers.take_scheme();
        let protocol = pseudo_headers.take_protocol();
        let request_headers = field_section.into_headers().into_owned();

        if let Some(host) = request_headers.get_str(KnownHeaderName::Host)
            && let Some(authority) = &authority
            && !authority_matches_host(authority, host, scheme.as_deref())
        {
            return None;
        }

        if H1_ONLY_HEADERS
            .into_iter()
            .any(|name| request_headers.has_header(name))
        {
            return None;
        }

        let method = method?;

        if method != Method::Connect && scheme.is_none() {
            return None;
        }

        let path = match (method, path) {
            (_, Some(path)) if !path.is_empty() => path,
            (Method::Connect, _) => Cow::Borrowed("/"),
            _ => return None,
        };

        if method == Method::Connect && authority.is_none() {
            return None;
        }

        // When :scheme names a scheme with a mandatory authority component, the request
        // MUST carry either :authority or a Host header. The spec gives "http" and "https"
        // as the canonical examples. Exotic schemes without mandatory authority
        // (file, data, mailto, urn) are exempt; CONNECT is handled above.
        if method != Method::Connect
            && matches!(scheme.as_deref(), Some("http" | "https"))
            && authority.is_none()
            && request_headers.get_str(KnownHeaderName::Host).is_none()
        {
            return None;
        }

        match request_headers.get_str(KnownHeaderName::Te) {
            None | Some("trailers") => {}
            _ => return None,
        }

        Some(ValidatedRequest {
            method,
            path,
            authority,
            scheme,
            protocol,
            request_headers,
        })
    }
}
