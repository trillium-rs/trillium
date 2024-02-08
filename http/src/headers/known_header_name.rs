use super::{HeaderName, HeaderNameInner};
use std::{
    fmt::{self, Debug, Display, Formatter},
    hash::Hash,
    str::FromStr,
};
use HeaderNameInner::{KnownHeader, UnknownHeader};

impl Display for KnownHeaderName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl From<KnownHeaderName> for HeaderName<'_> {
    fn from(khn: KnownHeaderName) -> Self {
        Self(KnownHeader(khn))
    }
}

impl PartialEq<HeaderName<'_>> for KnownHeaderName {
    fn eq(&self, other: &HeaderName) -> bool {
        match &other.0 {
            KnownHeader(k) => self == k,
            UnknownHeader(_) => false,
        }
    }
}

macro_rules! known_headers {
    (
        $(
            ($capitalized:literal, $variant:tt)
        ),+
    ) => {

        /// A short nonehaustive enum of headers that trillium can
        /// represent as a u8. Use a `KnownHeaderName` variant instead
        /// of a &'static str anywhere possible, as it allows trillium
        /// to skip parsing the header entirely.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        #[repr(u8)]
        pub enum KnownHeaderName {
            $(
                #[doc = concat!("The [", $capitalized, "](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/", $capitalized, ") header.")]
                $variant,
            )+
        }


        impl AsRef<str> for KnownHeaderName {
            fn as_ref(&self) -> &str {
                match self {
                    $( Self::$variant => $capitalized, )+
                }
            }
        }

        impl FromStr for KnownHeaderName {
            type Err = ();
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                if !s.is_ascii() { return Err(()); }

                $( if s.eq_ignore_ascii_case($capitalized) { Ok(Self::$variant) } else )+
                { Err(()) }
            }
        }
    }
}

/* generated with

console.log($$('main > article > div > dl > dt > a > code').map(code => {
let lowered = code.innerText.toLowerCase();
let enum_ = lowered.replace(/(?:-|^)([a-z])/g, (_, p1) => p1.toUpperCase());
return`("${code.innerText}", ${enum_}, "${lowered}")`
}).join(",\n"))

 on https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers


per https://httpwg.org/specs/rfc9110.html#rfc.section.5.3,

The order in which field lines with differing field names are received in a section is not
significant. However, it is good practice to send header fields that contain additional control data
first, such as Host on requests and Date on responses, so that implementations can decide when not
to handle a message as early as possible.
*/
known_headers! {
    ("Host", Host),
    ("Date", Date),

    ("Accept", Accept),
    ("Accept-CH", AcceptCh),
    ("Accept-CH-Lifetime", AcceptChLifetime),
    ("Accept-Charset", AcceptCharset),
    ("Accept-Encoding", AcceptEncoding),
    ("Accept-Language", AcceptLanguage),
    ("Accept-Push-Policy", AcceptPushPolicy),
    ("Accept-Ranges", AcceptRanges),
    ("Accept-Signature", AcceptSignature),
    ("Access-Control-Allow-Credentials", AccessControlAllowCredentials),
    ("Access-Control-Allow-Headers", AccessControlAllowHeaders),
    ("Access-Control-Allow-Methods", AccessControlAllowMethods),
    ("Access-Control-Allow-Origin", AccessControlAllowOrigin),
    ("Access-Control-Expose-Headers", AccessControlExposeHeaders),
    ("Access-Control-Max-Age", AccessControlMaxAge),
    ("Access-Control-Request-Headers", AccessControlRequestHeaders),
    ("Access-Control-Request-Method", AccessControlRequestMethod),
    ("Age", Age),
    ("Allow", Allow),
    ("Alt-Svc", AltSvc),
    ("Authorization", Authorization),
    ("Cache-Control", CacheControl),
    ("Clear-Site-Data", ClearSiteData),
    ("Connection", Connection),
    ("Content-DPR", ContentDpr),
    ("Content-Disposition", ContentDisposition),
    ("Content-Encoding", ContentEncoding),
    ("Content-Language", ContentLanguage),
    ("Content-Length", ContentLength),
    ("Content-Location", ContentLocation),
    ("Content-Range", ContentRange),
    ("Content-Security-Policy", ContentSecurityPolicy),
    ("Content-Security-Policy-Report-Only", ContentSecurityPolicyReportOnly),
    ("Content-Type", ContentType),
    ("Cookie", Cookie),
    ("Cookie2", Cookie2),
    ("Cross-Origin-Embedder-Policy", CrossOriginEmbedderPolicy),
    ("Cross-Origin-Opener-Policy", CrossOriginOpenerPolicy),
    ("Cross-Origin-Resource-Policy", CrossOriginResourcePolicy),
    ("DNT", Dnt),
    ("DPR", Dpr),
    ("Device-Memory", DeviceMemory),
    ("Downlink", Downlink),
    ("ECT", Ect),
    ("ETag", Etag),
    ("Early-Data", EarlyData),
    ("Expect", Expect),
    ("Expect-CT", ExpectCt),
    ("Expires", Expires),
    ("Feature-Policy", FeaturePolicy),
    ("Forwarded", Forwarded),
    ("From", From),
    ("If-Match", IfMatch),
    ("If-Modified-Since", IfModifiedSince),
    ("If-None-Match", IfNoneMatch),
    ("If-Range", IfRange),
    ("If-Unmodified-Since", IfUnmodifiedSince),
    ("Keep-Alive", KeepAlive),
    ("Large-Allocation", LargeAllocation),
    ("Last-Event-ID", LastEventId),
    ("Last-Modified", LastModified),
    ("Link", Link),
    ("Location", Location),
    ("Max-Forwards", MaxForwards),
    ("NEL", Nel),
    ("Origin", Origin),
    ("Origin-Isolation", OriginIsolation),
    ("Ping-From", PingFrom),
    ("Ping-To", PingTo),
    ("Pragma", Pragma),
    ("Proxy-Authenticate", ProxyAuthenticate),
    ("Proxy-Authorization", ProxyAuthorization),
    ("Proxy-Connection", ProxyConnection),
    ("Public-Key-Pins", PublicKeyPins),
    ("Public-Key-Pins-Report-Only", PublicKeyPinsReportOnly),
    ("Push-Policy", PushPolicy),
    ("RTT", Rtt),
    ("Range", Range),
    ("Referer", Referer),
    ("Referrer-Policy", ReferrerPolicy),
    ("Refresh-Cache", RefreshCache),
    ("Report-To", ReportTo),
    ("Retry-After", RetryAfter),
    ("Save-Data", SaveData),
    ("Sec-CH-UA", SecChUa),
    ("Sec-CH-UA-Mobile", SecChUAMobile),
    ("Sec-CH-UA-Platform", SecChUAPlatform),
    ("Sec-Fetch-Dest", SecFetchDest),
    ("Sec-Fetch-Mode", SecFetchMode),
    ("Sec-Fetch-Site", SecFetchSite),
    ("Sec-Fetch-User", SecFetchUser),
    ("Sec-GPC", SecGpc),
    ("Sec-WebSocket-Accept", SecWebsocketAccept),
    ("Sec-WebSocket-Extensions", SecWebsocketExtensions),
    ("Sec-WebSocket-Key", SecWebsocketKey),
    ("Sec-WebSocket-Protocol", SecWebsocketProtocol),
    ("Sec-WebSocket-Version", SecWebsocketVersion),
    ("Server", Server),
    ("Server-Timing", ServerTiming),
    ("Service-Worker-Allowed", ServiceWorkerAllowed),
    ("Set-Cookie", SetCookie),
    ("Set-Cookie2", SetCookie2),
    ("Signature", Signature),
    ("Signed-Headers", SignedHeaders),
    ("SourceMap", Sourcemap),
    ("Strict-Transport-Security", StrictTransportSecurity),
    ("TE", Te),
    ("Timing-Allow-Origin", TimingAllowOrigin),
    ("Trailer", Trailer),
    ("Transfer-Encoding", TransferEncoding),
    ("Upgrade", Upgrade),
    ("Upgrade-Insecure-Requests", UpgradeInsecureRequests),
    ("User-Agent", UserAgent),
    ("Vary", Vary),
    ("Via", Via),
    ("Viewport-Width", ViewportWidth),
    ("WWW-Authenticate", WwwAuthenticate),
    ("Warning", Warning),
    ("Width", Width),
    ("X-Cache", Xcache),
    ("X-Content-Type-Options", XcontentTypeOptions),
    ("X-DNS-Prefetch-Control", XdnsPrefetchControl),
    ("X-Download-Options", XdownloadOptions),
    ("X-Firefox-Spdy", XfirefoxSpdy),
    ("X-Forwarded-By", XforwardedBy),
    ("X-Forwarded-For", XforwardedFor),
    ("X-Forwarded-Host", XforwardedHost),
    ("X-Forwarded-Proto", XforwardedProto),
    ("X-Forwarded-SSL", XforwardedSsl),
    ("X-Frame-Options", XframeOptions),
    ("X-Permitted-Cross-Domain-Policies", XpermittedCrossDomainPolicies),
    ("X-Pingback", Xpingback),
    ("X-Powered-By", XpoweredBy),
    ("X-Request-Id", XrequestId),
    ("X-Requested-With", XrequestedWith),
    ("X-Robots-Tag", XrobotsTag),
    ("X-Served-By", XservedBy),
    ("X-UA-Compatible", XuaCompatible),
    ("X-XSS-Protection", XxssProtection)
}
