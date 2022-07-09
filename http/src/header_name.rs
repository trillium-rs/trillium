use smartcow::SmartCow;
use smartstring::alias::String as SmartString;
use std::{
    fmt::{self, Display, Formatter},
    hash::Hash,
    str::FromStr,
};

/// The name of a http header. This can be either a
/// [`KnownHeaderName`] or a string representation of an unknown
/// header.
#[derive(Clone, Debug)]
pub struct HeaderName<'a>(pub(crate) HeaderNameInner<'a>);

#[derive(Clone, Debug)]
pub(crate) enum HeaderNameInner<'a> {
    /// A `KnownHeaderName`
    KnownHeader(KnownHeaderName),
    UnknownHeader(SmartCow<'a>),
}
use crate::Error;
use HeaderNameInner::{KnownHeader, UnknownHeader};

impl<'a> HeaderName<'a> {
    /// Convert a potentially-borrowed headername to a static
    /// headername _by value_.
    #[must_use]
    pub fn into_owned(self) -> HeaderName<'static> {
        HeaderName(match self.0 {
            KnownHeader(known) => KnownHeader(known),
            UnknownHeader(smartcow) => UnknownHeader(smartcow.into_owned()),
        })
    }

    /// Convert a potentially-borrowed headername to a static
    /// headername _by cloning if needed from a borrow_. If you have
    /// ownership of a headername with a non-static lifetime, it is
    /// preferable to use `into_owned`. This is the equivalent of
    /// `self.clone().into_owned()`.
    #[must_use]
    pub fn to_owned(&self) -> HeaderName<'static> {
        self.clone().into_owned()
    }
}

impl PartialEq for HeaderName<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (KnownHeader(kh1), KnownHeader(kh2)) => *kh1 == *kh2,
            (UnknownHeader(u1), UnknownHeader(u2)) => u1.eq_ignore_ascii_case(u2),
            _ => false,
        }
    }
}

impl PartialEq<KnownHeaderName> for HeaderName<'_> {
    fn eq(&self, other: &KnownHeaderName) -> bool {
        match &self.0 {
            KnownHeader(k) => other == k,
            UnknownHeader(_) => false,
        }
    }
}

impl PartialEq<KnownHeaderName> for &HeaderName<'_> {
    fn eq(&self, other: &KnownHeaderName) -> bool {
        match &self.0 {
            KnownHeader(k) => other == k,
            UnknownHeader(_) => false,
        }
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

impl Hash for HeaderName<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match &self.0 {
            KnownHeader(k) => k.hash(state),
            UnknownHeader(u) => {
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
        Self(match s.parse::<KnownHeaderName>() {
            Ok(khn) => KnownHeader(khn),
            Err(()) => UnknownHeader(SmartCow::Owned(s.into())),
        })
    }
}

impl<'a> From<&'a str> for HeaderName<'a> {
    fn from(s: &'a str) -> Self {
        Self(match s.parse::<KnownHeaderName>() {
            Ok(khn) => KnownHeader(khn),
            Err(_e) => UnknownHeader(SmartCow::Borrowed(s)),
        })
    }
}

impl From<KnownHeaderName> for HeaderName<'_> {
    fn from(khn: KnownHeaderName) -> Self {
        Self(KnownHeader(khn))
    }
}

impl FromStr for HeaderName<'static> {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_ascii() {
            Ok(Self(match s.parse::<KnownHeaderName>() {
                Ok(known) => KnownHeader(known),
                Err(_) => UnknownHeader(SmartCow::Owned(SmartString::from(s))),
            }))
        } else {
            Err(Error::MalformedHeader(s.to_string().into()))
        }
    }
}

impl AsRef<str> for HeaderName<'_> {
    fn as_ref(&self) -> &str {
        match &self.0 {
            KnownHeader(khn) => khn.as_ref(),
            UnknownHeader(u) => u.as_ref(),
        }
    }
}

impl Display for HeaderName<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl Display for KnownHeaderName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
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
