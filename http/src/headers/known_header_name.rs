use super::{HeaderName, HeaderNameInner};
use std::{
    fmt::{self, Debug, Display, Formatter},
    hash::Hash,
    str::FromStr,
};

impl Display for KnownHeaderName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl From<KnownHeaderName> for HeaderName<'_> {
    fn from(khn: KnownHeaderName) -> Self {
        Self(HeaderNameInner::KnownHeader(khn))
    }
}

impl PartialEq<HeaderName<'_>> for KnownHeaderName {
    fn eq(&self, other: &HeaderName) -> bool {
        matches!(&other.0, HeaderNameInner::KnownHeader(k) if self == k)
    }
}

impl AsRef<str> for KnownHeaderName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

macro_rules! known_headers {
    (
        $(
            ($capitalized:literal, $variant:tt, $lower:literal)
        ),+
    ) => {

        /// A short nonehaustive enum of headers that trillium can
        /// represent as a u8. Use a `KnownHeaderName` variant instead
        /// of a &'static str anywhere possible, as it allows trillium
        /// to skip parsing the header entirely.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
        #[non_exhaustive]
        #[repr(u8)]
        pub enum KnownHeaderName {
            $(
                #[doc = concat!("The [", $capitalized, "](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/", $capitalized, ") header.")]
                $variant,
            )+
        }

        impl KnownHeaderName {
            /// Retrieve a static string representation of this header name
            pub fn as_str(&self) -> &'static str {
                match self {
                    $( Self::$variant => $capitalized, )+
                }
            }

            /// Retrieve a lowercase static string representation of this header name
            pub fn as_lower_str(&self) -> &'static str {
                match self {
                    $( Self::$variant => $lower, )+
                }
            }
        }

        impl FromStr for KnownHeaderName {
            type Err = ();
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                if !s.is_ascii() { return Err(()); }
                let len = s.len();

                $( if len == $capitalized.len() && s.eq_ignore_ascii_case($capitalized) { return Ok(Self::$variant); } )+
                Err(())
            }
        }

        #[cfg(test)]
        mod known_header_name_tests {
            use super::*;

            #[test]
            fn roundtrip_all_variants() {
                $(
                    let parsed: KnownHeaderName = $capitalized.parse()
                        .unwrap_or_else(|_| panic!("failed to parse {:?}", $capitalized));
                    assert_eq!(
                        parsed,
                        KnownHeaderName::$variant,
                        "parse({:?}) returned wrong variant",
                        $capitalized,
                    );
                    assert_eq!(
                        parsed.as_str(),
                        $capitalized,
                        "as_str() mismatch for {:?}",
                        stringify!($variant),
                    );
                )+
            }

            #[test]
            fn roundtrip_all_lower_variants() {
                $(
                    let parsed: KnownHeaderName = $lower.parse()
                        .unwrap_or_else(|_| panic!("failed to parse {:?}", $lower));
                    assert_eq!(
                        parsed,
                        KnownHeaderName::$variant,
                        "parse({:?}) returned wrong variant",
                        $lower,
                    );
                    assert_eq!(
                        parsed.as_lower_str(),
                        $lower,
                        "as_str() mismatch for {:?}",
                        stringify!($variant),
                    );
                )+
            }


            #[test]
            fn case_insensitive_roundtrip() {
                $(
                    let lower: KnownHeaderName = $capitalized.to_lowercase().parse()
                        .unwrap_or_else(|_| panic!("failed to parse lowercase {:?}", $capitalized));
                    assert_eq!(lower, KnownHeaderName::$variant);

                    let upper: KnownHeaderName = $capitalized.to_uppercase().parse()
                        .unwrap_or_else(|_| panic!("failed to parse uppercase {:?}", $capitalized));
                    assert_eq!(upper, KnownHeaderName::$variant);
                )+
            }

            #[test]
            fn unknown_headers_return_err() {
                assert!("X-Unknown-Custom-Header".parse::<KnownHeaderName>().is_err());
                assert!("".parse::<KnownHeaderName>().is_err());
                assert!("Hostt".parse::<KnownHeaderName>().is_err());
                assert!("Hos".parse::<KnownHeaderName>().is_err());
            }
        }
    }
}

// generated with
//
// console.log($$('main > article > div > dl > dt > a > code').map(code => {
// let lowered = code.innerText.toLowerCase();
// let enum_ = lowered.replace(/(?:-|^)([a-z])/g, (_, p1) => p1.toUpperCase());
// return`("${code.innerText}", ${enum_}, "${lowered}")`
// }).join(",\n"))
//
// on https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers
//
//
// per https://httpwg.org/specs/rfc9110.html#rfc.section.5.3,
//
// The order in which field lines with differing field names are received in a section is not
// significant. However, it is good practice to send header fields that contain additional control
// data first, such as Host on requests and Date on responses, so that implementations can decide
// when not to handle a message as early as possible.
known_headers! {
    ("Host", Host, "host"),
    ("Date", Date, "date"),

    ("Accept", Accept, "accept"),
    ("Accept-CH", AcceptCh, "accept-ch"),
    ("Accept-CH-Lifetime", AcceptChLifetime, "accept-ch-lifetime"),
    ("Accept-Charset", AcceptCharset, "accept-charset"),
    ("Accept-Encoding", AcceptEncoding, "accept-encoding"),
    ("Accept-Language", AcceptLanguage, "accept-language"),
    ("Accept-Push-Policy", AcceptPushPolicy, "accept-push-policy"),
    ("Accept-Ranges", AcceptRanges, "accept-ranges"),
    ("Accept-Signature", AcceptSignature, "accept-signature"),
    ("Access-Control-Allow-Credentials", AccessControlAllowCredentials, "access-control-allow-credentials"),
    ("Access-Control-Allow-Headers", AccessControlAllowHeaders, "access-control-allow-headers"),
    ("Access-Control-Allow-Methods", AccessControlAllowMethods, "access-control-allow-methods"),
    ("Access-Control-Allow-Origin", AccessControlAllowOrigin, "access-control-allow-origin"),
    ("Access-Control-Expose-Headers", AccessControlExposeHeaders, "access-control-expose-headers"),
    ("Access-Control-Max-Age", AccessControlMaxAge, "access-control-max-age"),
    ("Access-Control-Request-Headers", AccessControlRequestHeaders, "access-control-request-headers"),
    ("Access-Control-Request-Method", AccessControlRequestMethod, "access-control-request-method"),
    ("Age", Age, "age"),
    ("Allow", Allow, "allow"),
    ("Alt-Svc", AltSvc, "alt-svc"),
    ("Alt-Used", AltUsed, "alt-used"),
    ("Authorization", Authorization, "authorization"),
    ("Cache-Control", CacheControl, "cache-control"),
    ("Clear-Site-Data", ClearSiteData, "clear-site-data"),
    ("Connection", Connection, "connection"),
    ("Content-DPR", ContentDpr, "content-dpr"),
    ("Content-Digest", ContentDigest, "content-digest"),
    ("Content-Disposition", ContentDisposition, "content-disposition"),
    ("Content-Encoding", ContentEncoding, "content-encoding"),
    ("Content-Language", ContentLanguage, "content-language"),
    ("Content-Length", ContentLength, "content-length"),
    ("Content-Location", ContentLocation, "content-location"),
    ("Content-Range", ContentRange, "content-range"),
    ("Content-Security-Policy", ContentSecurityPolicy, "content-security-policy"),
    ("Content-Security-Policy-Report-Only", ContentSecurityPolicyReportOnly, "content-security-policy-report-only"),
    ("Content-Type", ContentType, "content-type"),
    ("Cookie", Cookie, "cookie"),
    ("Cookie2", Cookie2, "cookie2"),
    ("Cross-Origin-Embedder-Policy", CrossOriginEmbedderPolicy, "cross-origin-embedder-policy"),
    ("Cross-Origin-Opener-Policy", CrossOriginOpenerPolicy, "cross-origin-opener-policy"),
    ("Cross-Origin-Resource-Policy", CrossOriginResourcePolicy, "cross-origin-resource-policy"),
    ("DNT", Dnt, "dnt"),
    ("DPR", Dpr, "dpr"),
    ("DPoP", Dpop, "dpop"),
    ("Deprecation", Deprecation, "deprecation"),
    ("Device-Memory", DeviceMemory, "device-memory"),
    ("Digest", Digest, "digest"),
    ("Downlink", Downlink, "downlink"),
    ("ECT", Ect, "ect"),
    ("ETag", Etag, "etag"),
    ("Early-Data", EarlyData, "early-data"),
    ("Expect", Expect, "expect"),
    ("Expect-CT", ExpectCt, "expect-ct"),
    ("Expires", Expires, "expires"),
    ("Feature-Policy", FeaturePolicy, "feature-policy"),
    ("Forwarded", Forwarded, "forwarded"),
    ("From", From, "from"),
    ("If-Match", IfMatch, "if-match"),
    ("If-Modified-Since", IfModifiedSince, "if-modified-since"),
    ("If-None-Match", IfNoneMatch, "if-none-match"),
    ("If-Range", IfRange, "if-range"),
    ("If-Unmodified-Since", IfUnmodifiedSince, "if-unmodified-since"),
    ("Keep-Alive", KeepAlive, "keep-alive"),
    ("Large-Allocation", LargeAllocation, "large-allocation"),
    ("Last-Event-ID", LastEventId, "last-event-id"),
    ("Last-Modified", LastModified, "last-modified"),
    ("Link", Link, "link"),
    ("Location", Location, "location"),
    ("Max-Forwards", MaxForwards, "max-forwards"),
    ("NEL", Nel, "nel"),
    ("Origin", Origin, "origin"),
    ("Origin-Isolation", OriginIsolation, "origin-isolation"),
    ("Permissions-Policy", PermissionsPolicy, "permissions-policy"),
    ("Ping-From", PingFrom, "ping-from"),
    ("Ping-To", PingTo, "ping-to"),
    ("Pragma", Pragma, "pragma"),
    ("Priority", Priority, "priority"),
    ("Proxy-Authenticate", ProxyAuthenticate, "proxy-authenticate"),
    ("Proxy-Authorization", ProxyAuthorization, "proxy-authorization"),
    ("Proxy-Connection", ProxyConnection, "proxy-connection"),
    ("Proxy-Status", ProxyStatus, "proxy-status"),
    ("Public-Key-Pins", PublicKeyPins, "public-key-pins"),
    ("Public-Key-Pins-Report-Only", PublicKeyPinsReportOnly, "public-key-pins-report-only"),
    ("Purpose", Purpose, "purpose"),
    ("Push-Policy", PushPolicy, "push-policy"),
    ("RTT", Rtt, "rtt"),
    ("Range", Range, "range"),
    ("RateLimit-Reset", RatelimitReset, "ratelimit-reset"),
    ("Ratelimit-Limit", RatelimitLimit, "ratelimit-limit"),
    ("Ratelimit-Remaining", RatelimitRemaining, "ratelimit-remaining"),
    ("Referer", Referer, "referer"),
    ("Referrer-Policy", ReferrerPolicy, "referrer-policy"),
    ("Refresh-Cache", RefreshCache, "refresh-cache"),
    ("Report-To", ReportTo, "report-to"),
    ("Repr-Digest", ReprDigest, "repr-digest"),
    ("Retry-After", RetryAfter, "retry-after"),
    ("Save-Data", SaveData, "save-data"),
    ("Sec-CH-UA", SecChUa, "sec-ch-ua"),
    ("Sec-CH-UA-Mobile", SecChUAMobile, "sec-ch-ua-mobile"),
    ("Sec-CH-UA-Platform", SecChUAPlatform, "sec-ch-ua-platform"),
    ("Sec-Fetch-Dest", SecFetchDest, "sec-fetch-dest"),
    ("Sec-Fetch-Mode", SecFetchMode, "sec-fetch-mode"),
    ("Sec-Fetch-Site", SecFetchSite, "sec-fetch-site"),
    ("Sec-Fetch-User", SecFetchUser, "sec-fetch-user"),
    ("Sec-GPC", SecGpc, "sec-gpc"),
    ("Sec-WebSocket-Accept", SecWebsocketAccept, "sec-websocket-accept"),
    ("Sec-WebSocket-Extensions", SecWebsocketExtensions, "sec-websocket-extensions"),
    ("Sec-WebSocket-Key", SecWebsocketKey, "sec-websocket-key"),
    ("Sec-WebSocket-Protocol", SecWebsocketProtocol, "sec-websocket-protocol"),
    ("Sec-WebSocket-Version", SecWebsocketVersion, "sec-websocket-version"),
    ("Server", Server, "server"),
    ("Server-Timing", ServerTiming, "server-timing"),
    ("Service-Worker-Allowed", ServiceWorkerAllowed, "service-worker-allowed"),
    ("Set-Cookie", SetCookie, "set-cookie"),
    ("Set-Cookie2", SetCookie2, "set-cookie2"),
    ("Signature", Signature, "signature"),
    ("Signed-Headers", SignedHeaders, "signed-headers"),
    ("SourceMap", Sourcemap, "sourcemap"),
    ("Strict-Transport-Security", StrictTransportSecurity, "strict-transport-security"),
    ("TE", Te, "te"),
    ("Timing-Allow-Origin", TimingAllowOrigin, "timing-allow-origin"),
    ("Traceparent", Traceparent, "traceparent"),
    ("Tracestate", Tracestate, "tracestate"),
    ("Trailer", Trailer, "trailer"),
    ("Transfer-Encoding", TransferEncoding, "transfer-encoding"),
    ("Upgrade", Upgrade, "upgrade"),
    ("Upgrade-Insecure-Requests", UpgradeInsecureRequests, "upgrade-insecure-requests"),
    ("User-Agent", UserAgent, "user-agent"),
    ("Vary", Vary, "vary"),
    ("Via", Via, "via"),
    ("Viewport-Width", ViewportWidth, "viewport-width"),
    ("WWW-Authenticate", WwwAuthenticate, "www-authenticate"),
    ("Want-Content-Digest", WantContentDigest, "want-content-digest"),
    ("Want-Digest", WantDigest, "want-digest"),
    ("Want-Repr-Digest", WantReprDigest, "want-repr-digest"),
    ("Warning", Warning, "warning"),
    ("Width", Width, "width"),
    ("X-B3-Traceid", Xb3Traceid, "x-b3-traceid"),
    ("X-Cache", Xcache, "x-cache"),
    ("X-Content-Type-Options", XcontentTypeOptions, "x-content-type-options"),
    ("X-Correlation-ID", XcorrelationId, "x-correlation-id"),
    ("X-DNS-Prefetch-Control", XdnsPrefetchControl, "x-dns-prefetch-control"),
    ("X-Download-Options", XdownloadOptions, "x-download-options"),
    ("X-Firefox-Spdy", XfirefoxSpdy, "x-firefox-spdy"),
    ("X-Forwarded-By", XforwardedBy, "x-forwarded-by"),
    ("X-Forwarded-For", XforwardedFor, "x-forwarded-for"),
    ("X-Forwarded-Host", XforwardedHost, "x-forwarded-host"),
    ("X-Forwarded-Proto", XforwardedProto, "x-forwarded-proto"),
    ("X-Forwarded-SSL", XforwardedSsl, "x-forwarded-ssl"),
    ("X-Frame-Options", XframeOptions, "x-frame-options"),
    ("X-Permitted-Cross-Domain-Policies", XpermittedCrossDomainPolicies, "x-permitted-cross-domain-policies"),
    ("X-Pingback", Xpingback, "x-pingback"),
    ("X-Powered-By", XpoweredBy, "x-powered-by"),
    ("X-Real-IP", XrealIp, "x-real-ip"),
    ("X-Request-Id", XrequestId, "x-request-id"),
    ("X-Requested-With", XrequestedWith, "x-requested-with"),
    ("X-Robots-Tag", XrobotsTag, "x-robots-tag"),
    ("X-Runtime", Xruntime, "x-runtime"),
    ("X-Served-By", XservedBy, "x-served-by"),
    ("X-UA-Compatible", XuaCompatible, "x-ua-compatible"),
    ("X-XSS-Protection", XxssProtection, "x-xss-protection")
}
