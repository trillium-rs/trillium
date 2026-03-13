use super::decoder::DecoderError;
use crate::KnownHeaderName::{
    self, Accept, AcceptEncoding, AcceptLanguage, AcceptRanges, AccessControlAllowCredentials,
    AccessControlAllowHeaders, AccessControlAllowMethods, AccessControlAllowOrigin,
    AccessControlExposeHeaders, AccessControlRequestHeaders, AccessControlRequestMethod, Age,
    AltSvc, Authorization, CacheControl, ContentDisposition, ContentEncoding, ContentLength,
    ContentSecurityPolicy, ContentType, Cookie, Date, EarlyData, Etag, ExpectCt, Forwarded,
    IfModifiedSince, IfNoneMatch, IfRange, LastModified, Link, Location, Origin, Purpose, Range,
    Referer, Server, SetCookie, StrictTransportSecurity, TimingAllowOrigin,
    UpgradeInsecureRequests, UserAgent, Vary, XcontentTypeOptions, XforwardedFor, XframeOptions,
    XxssProtection,
};
use PseudoHeaderName::{Authority, Method, Path, Scheme, Status};
use StaticHeaderName::{Header, Pseudo};
use core::{
    convert::AsRef,
    fmt::{self, Display, Formatter},
};
mod lookup;
pub(super) use lookup::{StaticLookup, static_table_lookup};

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(crate) enum StaticHeaderName {
    Header(KnownHeaderName),
    Pseudo(PseudoHeaderName),
}

impl StaticHeaderName {
    /// Retrieve a 'static str representation
    pub fn as_str(self) -> &'static str {
        match self {
            Header(known_header_name) => known_header_name.as_str(),
            Pseudo(pseudo_header) => pseudo_header.as_str(),
        }
    }
}

impl AsRef<str> for StaticHeaderName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl Display for StaticHeaderName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Eq, PartialEq, Hash, Debug)]
pub(crate) enum PseudoHeaderName {
    Authority,
    Method,
    Path,
    Scheme,
    Status,
}

impl PseudoHeaderName {
    /// Retrieve a 'static str representation
    pub fn as_str(self) -> &'static str {
        match self {
            Authority => ":authority",
            Method => ":method",
            Path => ":path",
            Scheme => ":scheme",
            Status => ":status",
        }
    }
}

impl Display for PseudoHeaderName {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub(crate) fn static_entry(
    index: usize,
) -> Result<&'static (StaticHeaderName, &'static str), DecoderError> {
    STATIC_TABLE
        .get(index)
        .ok_or(DecoderError::InvalidStaticIndex(index))
}

const STATIC_TABLE: [(StaticHeaderName, &str); 99] = [
    (Pseudo(Authority), ""),
    (Pseudo(Path), "/"),
    (Header(Age), "0"),
    (Header(ContentDisposition), ""),
    (Header(ContentLength), "0"),
    (Header(Cookie), ""),
    (Header(Date), ""),
    (Header(Etag), ""),
    (Header(IfModifiedSince), ""),
    (Header(IfNoneMatch), ""),
    (Header(LastModified), ""),
    (Header(Link), ""),
    (Header(Location), ""),
    (Header(Referer), ""),
    (Header(SetCookie), ""),
    (Pseudo(Method), "CONNECT"),
    (Pseudo(Method), "DELETE"),
    (Pseudo(Method), "GET"),
    (Pseudo(Method), "HEAD"),
    (Pseudo(Method), "OPTIONS"),
    (Pseudo(Method), "POST"),
    (Pseudo(Method), "PUT"),
    (Pseudo(Scheme), "http"),
    (Pseudo(Scheme), "https"),
    (Pseudo(Status), "103"),
    (Pseudo(Status), "200"),
    (Pseudo(Status), "304"),
    (Pseudo(Status), "404"),
    (Pseudo(Status), "503"),
    (Header(Accept), "*/*"),
    (Header(Accept), "application/dns-message"),
    (Header(AcceptEncoding), "gzip, deflate, br"),
    (Header(AcceptRanges), "bytes"),
    (Header(AccessControlAllowHeaders), "cache-control"),
    (Header(AccessControlAllowHeaders), "content-type"),
    (Header(AccessControlAllowOrigin), "*"),
    (Header(CacheControl), "max-age=0"),
    (Header(CacheControl), "max-age=2592000"),
    (Header(CacheControl), "max-age=604800"),
    (Header(CacheControl), "no-cache"),
    (Header(CacheControl), "no-store"),
    (Header(CacheControl), "public, max-age=31536000"),
    (Header(ContentEncoding), "br"),
    (Header(ContentEncoding), "gzip"),
    (Header(ContentType), "application/dns-message"),
    (Header(ContentType), "application/javascript"),
    (Header(ContentType), "application/json"),
    (Header(ContentType), "application/x-www-form-urlencoded"),
    (Header(ContentType), "image/gif"),
    (Header(ContentType), "image/jpeg"),
    (Header(ContentType), "image/png"),
    (Header(ContentType), "text/css"),
    (Header(ContentType), "text/html;charset=utf-8"),
    (Header(ContentType), "text/plain"),
    (Header(ContentType), "text/plain;charset=utf-8"),
    (Header(Range), "bytes=0-"),
    (Header(StrictTransportSecurity), "max-age=31536000"),
    (
        Header(StrictTransportSecurity),
        "max-age=31536000;includesubdomains",
    ),
    (
        Header(StrictTransportSecurity),
        "max-age=31536000;includesubdomains;preload",
    ),
    (Header(Vary), "accept-encoding"),
    (Header(Vary), "origin"),
    (Header(XcontentTypeOptions), "nosniff"),
    (Header(XxssProtection), "1; mode=block"),
    (Pseudo(Status), "100"),
    (Pseudo(Status), "204"),
    (Pseudo(Status), "206"),
    (Pseudo(Status), "302"),
    (Pseudo(Status), "400"),
    (Pseudo(Status), "403"),
    (Pseudo(Status), "421"),
    (Pseudo(Status), "425"),
    (Pseudo(Status), "500"),
    (Header(AcceptLanguage), ""),
    (Header(AccessControlAllowCredentials), "FALSE"),
    (Header(AccessControlAllowCredentials), "TRUE"),
    (Header(AccessControlAllowHeaders), "*"),
    (Header(AccessControlAllowMethods), "get"),
    (Header(AccessControlAllowMethods), "get, post, options"),
    (Header(AccessControlAllowMethods), "options"),
    (Header(AccessControlExposeHeaders), "content-length"),
    (Header(AccessControlRequestHeaders), "content-type"),
    (Header(AccessControlRequestMethod), "get"),
    (Header(AccessControlRequestMethod), "post"),
    (Header(AltSvc), "clear"),
    (Header(Authorization), ""),
    (
        Header(ContentSecurityPolicy),
        "script-src 'none';object-src 'none';base-uri 'none'",
    ),
    (Header(EarlyData), "1"),
    (Header(ExpectCt), ""),
    (Header(Forwarded), ""),
    (Header(IfRange), ""),
    (Header(Origin), ""),
    (Header(Purpose), "prefetch"),
    (Header(Server), ""),
    (Header(TimingAllowOrigin), "*"),
    (Header(UpgradeInsecureRequests), "1"),
    (Header(UserAgent), ""),
    (Header(XforwardedFor), ""),
    (Header(XframeOptions), "deny"),
    (Header(XframeOptions), "sameorigin"),
];
