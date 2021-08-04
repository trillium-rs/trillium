#![allow(nonstandard_style)]

use cervine::Cow;
use smartstring::alias::String as SmartString;
use std::{fmt::Display, hash::Hash, str::FromStr};

#[derive(Clone, Debug)]
pub enum HeaderName<'a> {
    KnownHeader(KnownHeaderName),
    UnknownHeader(Cow<'a, SmartString, str>),
}

impl<'a> HeaderName<'a> {
    fn to_owned(&self) -> HeaderName<'static> {
        match self {
            HeaderName::KnownHeader(known) => HeaderName::KnownHeader(*known),

            HeaderName::UnknownHeader(Cow::Owned(o)) => {
                HeaderName::UnknownHeader(Cow::Owned(o.clone()))
            }

            HeaderName::UnknownHeader(Cow::Borrowed(b)) => {
                HeaderName::UnknownHeader(Cow::Owned(SmartString::from(*b)))
            }
        }
    }
}

impl PartialEq for HeaderName<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (HeaderName::KnownHeader(kh1), HeaderName::KnownHeader(kh2)) => kh1 == kh2,
            (HeaderName::UnknownHeader(u1), HeaderName::UnknownHeader(u2)) => {
                u1.eq_ignore_ascii_case(u2)
            }
            _ => false,
        }
    }
}

impl PartialEq<KnownHeaderName> for HeaderName<'_> {
    fn eq(&self, other: &KnownHeaderName) -> bool {
        match self {
            HeaderName::KnownHeader(k) => other == k,
            _ => false,
        }
    }
}

impl PartialEq<KnownHeaderName> for &HeaderName<'_> {
    fn eq(&self, other: &KnownHeaderName) -> bool {
        match self {
            HeaderName::KnownHeader(k) => other == k,
            _ => false,
        }
    }
}

impl PartialEq<HeaderName<'_>> for KnownHeaderName {
    fn eq(&self, other: &HeaderName) -> bool {
        match other {
            HeaderName::KnownHeader(k) => self == k,
            _ => false,
        }
    }
}

impl Hash for HeaderName<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            HeaderName::KnownHeader(k) => k.hash(state),
            HeaderName::UnknownHeader(u) => {
                for byte in u.bytes().map(|b| b.to_ascii_lowercase()) {
                    state.write_u8(byte);
                }
            }
        }
    }
}

impl Eq for HeaderName<'_> {}

impl From<String> for HeaderName<'static> {
    fn from(s: String) -> Self {
        match s.parse::<KnownHeaderName>() {
            Ok(khn) => Self::KnownHeader(khn),
            Err(()) => Self::UnknownHeader(Cow::Owned(s.into())),
        }
    }
}

impl<'a> From<&'a str> for HeaderName<'a> {
    fn from(s: &'a str) -> Self {
        match s.parse::<KnownHeaderName>() {
            Ok(khn) => Self::KnownHeader(khn),
            Err(_e) => Self::UnknownHeader(Cow::Borrowed(s)),
        }
    }
}

impl From<KnownHeaderName> for HeaderName<'_> {
    fn from(khn: KnownHeaderName) -> Self {
        Self::KnownHeader(khn)
    }
}

impl FromStr for HeaderName<'static> {
    type Err = crate::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_ascii() {
            match s.parse::<KnownHeaderName>() {
                Ok(known) => Ok(Self::KnownHeader(known)),
                Err(_) => Ok(Self::UnknownHeader(Cow::Owned(SmartString::from(s)))),
            }
        } else {
            Err(crate::Error::MalformedHeader(s.to_string().into()))
        }
    }
}

impl AsRef<str> for HeaderName<'_> {
    fn as_ref(&self) -> &str {
        match self {
            HeaderName::KnownHeader(khn) => khn.as_ref(),
            HeaderName::UnknownHeader(u) => u.as_ref(),
        }
    }
}

impl Display for HeaderName<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_ref())
    }
}

macro_rules! known_headers {
    (
        $(
            ($capitalized:literal, $variant:tt)
        ),+
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[allow(nonstandard_style)]
        #[repr(u8)]
        pub enum KnownHeaderName {
            $(
                #[doc = concat!("[", $capitalized, "](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/", $capitalized, ")")]
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

        impl Display for KnownHeaderName {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_ref())
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
*/
known_headers! {
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
    ("Date", Date),
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
    ("Host", Host),
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
    ("X-UA-Compatible", XuaCompatible),
    ("X-XSS-Protection", XxssProtection)
}
