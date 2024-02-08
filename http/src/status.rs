// originally from https://github.com/http-rs/http-types/blob/main/src/status_code.rs
use crate::Error;
use std::{
    convert::TryFrom,
    fmt::{self, Debug, Display},
    str::FromStr,
};

/// HTTP response status codes.
///
/// As defined by [rfc7231 section 6](https://tools.ietf.org/html/rfc7231#section-6).
/// [Read more](https://developer.mozilla.org/en-US/docs/Web/HTTP/Status)
#[repr(u16)]
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Status {
    /// 100 Continue
    ///
    /// This interim response indicates that everything so far is OK and that
    /// the client should continue the request, or ignore the response if
    /// the request is already finished.
    Continue = 100,

    /// 101 Switching Protocols
    ///
    /// This code is sent in response to an Upgrade request header from the
    /// client, and indicates the protocol the server is switching to.
    SwitchingProtocols = 101,

    /// 103 Early Hints
    ///
    /// This status code is primarily intended to be used with the Link header,
    /// letting the user agent start preloading resources while the server
    /// prepares a response.
    EarlyHints = 103,

    /// 200 Ok
    ///
    /// The request has succeeded
    Ok = 200,

    /// 201 Created
    ///
    /// The request has succeeded and a new resource has been created as a
    /// result. This is typically the response sent after POST requests, or
    /// some PUT requests.
    Created = 201,

    /// 202 Accepted
    ///
    /// The request has been received but not yet acted upon. It is
    /// noncommittal, since there is no way in HTTP to later send an
    /// asynchronous response indicating the outcome of the request. It is
    /// intended for cases where another process or server handles the request,
    /// or for batch processing.
    Accepted = 202,

    /// 203 Non Authoritative Information
    ///
    /// This response code means the returned meta-information is not exactly
    /// the same as is available from the origin server, but is collected
    /// from a local or a third-party copy. This is mostly used for mirrors
    /// or backups of another resource. Except for that specific case, the
    /// "200 OK" response is preferred to this status.
    NonAuthoritativeInformation = 203,

    /// 204 No Content
    ///
    /// There is no content to send for this request, but the headers may be
    /// useful. The user-agent may update its cached headers for this
    /// resource with the new ones.
    NoContent = 204,

    /// 205 Reset Content
    ///
    /// Tells the user-agent to reset the document which sent this request.
    ResetContent = 205,

    /// 206 Partial Content
    ///
    /// This response code is used when the Range header is sent from the client
    /// to request only part of a resource.
    PartialContent = 206,

    /// 207 Multi-Status
    ///
    /// A Multi-Status response conveys information about
    /// multiple resources in situations where multiple
    /// status codes might be appropriate.
    MultiStatus = 207,

    /// 226 Im Used
    ///
    /// The server has fulfilled a GET request for the resource, and the
    /// response is a representation of the result of one or more
    /// instance-manipulations applied to the current instance.
    ImUsed = 226,

    /// 300 Multiple Choice
    ///
    /// The request has more than one possible response. The user-agent or user
    /// should choose one of them. (There is no standardized way of choosing
    /// one of the responses, but HTML links to the possibilities are
    /// recommended so the user can pick.)
    MultipleChoice = 300,

    /// 301 Moved Permanently
    ///
    /// The URL of the requested resource has been changed permanently. The new
    /// URL is given in the response.
    MovedPermanently = 301,

    /// 302 Found
    ///
    /// This response code means that the URI of requested resource has been
    /// changed temporarily. Further changes in the URI might be made in the
    /// future. Therefore, this same URI should be used by the client in
    /// future requests.
    Found = 302,

    /// 303 See Other
    ///
    /// The server sent this response to direct the client to get the requested
    /// resource at another URI with a GET request.
    SeeOther = 303,

    /// 304 Not Modified
    ///
    /// This is used for caching purposes. It tells the client that the response
    /// has not been modified, so the client can continue to use the same
    /// cached version of the response.
    NotModified = 304,

    /// 307 Temporary Redirect
    ///
    /// The server sends this response to direct the client to get the requested
    /// resource at another URI with same method that was used in the prior
    /// request. This has the same semantics as the 302 Found HTTP response
    /// code, with the exception that the user agent must not change the
    /// HTTP method used: If a POST was used in the first request, a POST must
    /// be used in the second request.
    TemporaryRedirect = 307,

    /// 308 Permanent Redirect
    ///
    /// This means that the resource is now permanently located at another URI,
    /// specified by the Location: HTTP Response header. This has the same
    /// semantics as the 301 Moved Permanently HTTP response code, with the
    /// exception that the user agent must not change the HTTP method
    /// used: If a POST was used in the first request, a POST must be used in
    /// the second request.
    PermanentRedirect = 308,

    /// 400 Bad Request
    ///
    /// The server could not understand the request due to invalid syntax.
    BadRequest = 400,

    /// 401 Unauthorized
    ///
    /// Although the HTTP standard specifies "unauthorized", semantically this
    /// response means "unauthenticated". That is, the client must
    /// authenticate itself to get the requested response.
    Unauthorized = 401,

    /// 402 Payment Required
    ///
    /// This response code is reserved for future use. The initial aim for
    /// creating this code was using it for digital payment systems, however
    /// this status code is used very rarely and no standard convention
    /// exists.
    PaymentRequired = 402,

    /// 403 Forbidden
    ///
    /// The client does not have access rights to the content; that is, it is
    /// unauthorized, so the server is refusing to give the requested
    /// resource. Unlike 401, the client's identity is known to the server.
    Forbidden = 403,

    /// 404 Not Found
    ///
    /// The server can not find requested resource. In the browser, this means
    /// the URL is not recognized. In an API, this can also mean that the
    /// endpoint is valid but the resource itself does not exist. Servers
    /// may also send this response instead of 403 to hide the existence of
    /// a resource from an unauthorized client. This response code is probably
    /// the most famous one due to its frequent occurrence on the web.
    NotFound = 404,

    /// 405 Method Not Allowed
    ///
    /// The request method is known by the server but has been disabled and
    /// cannot be used. For example, an API may forbid DELETE-ing a
    /// resource. The two mandatory methods, GET and HEAD, must never be
    /// disabled and should not return this error code.
    MethodNotAllowed = 405,

    /// 406 Not Acceptable
    ///
    /// This response is sent when the web server, after performing
    /// server-driven content negotiation, doesn't find any content that
    /// conforms to the criteria given by the user agent.
    NotAcceptable = 406,

    /// 407 Proxy Authentication Required
    ///
    /// This is similar to 401 but authentication is needed to be done by a
    /// proxy.
    ProxyAuthenticationRequired = 407,

    /// 408 Request Timeout
    ///
    /// This response is sent on an idle connection by some servers, even
    /// without any previous request by the client. It means that the server
    /// would like to shut down this unused connection. This response is
    /// used much more since some browsers, like Chrome, Firefox 27+,
    /// or IE9, use HTTP pre-connection mechanisms to speed up surfing. Also
    /// note that some servers merely shut down the connection without
    /// sending this message.
    RequestTimeout = 408,

    /// 409 Conflict
    ///
    /// This response is sent when a request conflicts with the current state of
    /// the server.
    Conflict = 409,

    /// 410 Gone
    ///
    /// This response is sent when the requested content has been permanently
    /// deleted from server, with no forwarding address. Clients are
    /// expected to remove their caches and links to the resource. The HTTP
    /// specification intends this status code to be used for "limited-time,
    /// promotional services". APIs should not feel compelled to indicate
    /// resources that have been deleted with this status code.
    Gone = 410,

    /// 411 Length Required
    ///
    /// Server rejected the request because the Content-Length header field is
    /// not defined and the server requires it.
    LengthRequired = 411,

    /// 412 Precondition Failed
    ///
    /// The client has indicated preconditions in its headers which the server
    /// does not meet.
    PreconditionFailed = 412,

    /// 413 Payload Too Large
    ///
    /// Request entity is larger than limits defined by server; the server might
    /// close the connection or return an Retry-After header field.
    PayloadTooLarge = 413,

    /// 414 URI Too Long
    ///
    /// The URI requested by the client is longer than the server is willing to
    /// interpret.
    UriTooLong = 414,

    /// 415 Unsupported Media Type
    ///
    /// The media format of the requested data is not supported by the server,
    /// so the server is rejecting the request.
    UnsupportedMediaType = 415,

    /// 416 Requested Range Not Satisfiable
    ///
    /// The range specified by the Range header field in the request can't be
    /// fulfilled; it's possible that the range is outside the size of the
    /// target URI's data.
    RequestedRangeNotSatisfiable = 416,

    /// 417 Expectation Failed
    ///
    /// This response code means the expectation indicated by the Expect request
    /// header field can't be met by the server.
    ExpectationFailed = 417,
    ///
    /// 418 I'm a teapot
    ///
    /// The server refuses the attempt to brew coffee with a teapot.
    ImATeapot = 418,

    /// 421 Misdirected Request
    ///
    /// The request was directed at a server that is not able to produce a
    /// response. This can be sent by a server that is not configured to
    /// produce responses for the combination of scheme and authority that
    /// are included in the request URI.
    MisdirectedRequest = 421,

    /// 422 Unprocessable Entity
    ///
    /// The request was well-formed but was unable to be followed due to
    /// semantic errors.
    UnprocessableEntity = 422,

    /// 423 Locked
    ///
    /// The resource that is being accessed is locked.
    Locked = 423,

    /// 424 Failed Dependency
    ///
    /// The request failed because it depended on another request and that
    /// request failed (e.g., a PROPPATCH).
    FailedDependency = 424,

    /// 425 Too Early
    ///
    /// Indicates that the server is unwilling to risk processing a request that
    /// might be replayed.
    TooEarly = 425,

    /// 426 Upgrade Required
    ///
    /// The server refuses to perform the request using the current protocol but
    /// might be willing to do so after the client upgrades to a different
    /// protocol. The server sends an Upgrade header in a 426 response to
    /// indicate the required protocol(s).
    UpgradeRequired = 426,

    /// 428 Precondition Required
    ///
    /// The origin server requires the request to be conditional. This response
    /// is intended to prevent the 'lost update' problem, where a client
    /// GETs a resource's state, modifies it, and PUTs it back to the
    /// server, when meanwhile a third party has modified the state on the
    /// server, leading to a conflict.
    PreconditionRequired = 428,

    /// 429 Too Many Requests
    ///
    /// The user has sent too many requests in a given amount of time ("rate
    /// limiting").
    TooManyRequests = 429,

    /// 431 Request Header Fields Too Large
    ///
    /// The server is unwilling to process the request because its header fields
    /// are too large. The request may be resubmitted after reducing the
    /// size of the request header fields.
    RequestHeaderFieldsTooLarge = 431,

    /// 451 Unavailable For Legal Reasons
    ///
    /// The user-agent requested a resource that cannot legally be provided,
    /// such as a web page censored by a government.
    UnavailableForLegalReasons = 451,

    /// 500 Internal Server Error
    ///
    /// The server has encountered a situation it doesn't know how to handle.
    InternalServerError = 500,

    /// 501 Not Implemented
    ///
    /// The request method is not supported by the server and cannot be handled.
    /// The only methods that servers are required to support (and therefore
    /// that must not return this code) are GET and HEAD.
    NotImplemented = 501,

    /// 502 Bad Gateway
    ///
    /// This error response means that the server, while working as a gateway to
    /// get a response needed to handle the request, got an invalid
    /// response.
    BadGateway = 502,

    /// 503 Service Unavailable
    ///
    /// The server is not ready to handle the request. Common causes are a
    /// server that is down for maintenance or that is overloaded. Note that
    /// together with this response, a user-friendly page explaining the
    /// problem should be sent. This responses should be used for temporary
    /// conditions and the Retry-After: HTTP header should, if possible, contain
    /// the estimated time before the recovery of the service. The webmaster
    /// must also take care about the caching-related headers that are sent
    /// along with this response, as these temporary condition responses
    /// should usually not be cached.
    ServiceUnavailable = 503,

    /// 504 Gateway Timeout
    ///
    /// This error response is given when the server is acting as a gateway and
    /// cannot get a response in time.
    GatewayTimeout = 504,

    /// 505 HTTP Version Not Supported
    ///
    /// The HTTP version used in the request is not supported by the server.
    HttpVersionNotSupported = 505,

    /// 506 Variant Also Negotiates
    ///
    /// The server has an internal configuration error: the chosen variant
    /// resource is configured to engage in transparent content negotiation
    /// itself, and is therefore not a proper end point in the negotiation
    /// process.
    VariantAlsoNegotiates = 506,

    /// 507 Insufficient Storage
    ///
    /// The server is unable to store the representation needed to complete the
    /// request.
    InsufficientStorage = 507,

    /// 508 Loop Detected
    ///
    /// The server detected an infinite loop while processing the request.
    LoopDetected = 508,

    /// 510 Not Extended
    ///
    /// Further extensions to the request are required for the server to fulfil
    /// it.
    NotExtended = 510,

    /// 511 Network Authentication Required
    ///
    /// The 511 status code indicates that the client needs to authenticate to
    /// gain network access.
    NetworkAuthenticationRequired = 511,
}

impl Status {
    /// Returns `true` if the status code is `1xx` range.
    ///
    /// If this returns `true` it indicates that the request was received,
    /// continuing process.
    pub fn is_informational(&self) -> bool {
        let num: u16 = (*self).into();
        (100..200).contains(&num)
    }

    /// Returns `true` if the status code is the `2xx` range.
    ///
    /// If this returns `true` it indicates that the request was successfully
    /// received, understood, and accepted.
    pub fn is_success(&self) -> bool {
        let num: u16 = (*self).into();
        (200..300).contains(&num)
    }

    /// Returns `true` if the status code is the `3xx` range.
    ///
    /// If this returns `true` it indicates that further action needs to be
    /// taken in order to complete the request.
    pub fn is_redirection(&self) -> bool {
        let num: u16 = (*self).into();
        (300..400).contains(&num)
    }

    /// Returns `true` if the status code is the `4xx` range.
    ///
    /// If this returns `true` it indicates that the request contains bad syntax
    /// or cannot be fulfilled.
    pub fn is_client_error(&self) -> bool {
        let num: u16 = (*self).into();
        (400..500).contains(&num)
    }

    /// Returns `true` if the status code is the `5xx` range.
    ///
    /// If this returns `true` it indicates that the server failed to fulfill an
    /// apparently valid request.
    pub fn is_server_error(&self) -> bool {
        let num: u16 = (*self).into();
        (500..600).contains(&num)
    }

    /// The canonical reason for a given status code
    pub fn canonical_reason(&self) -> &'static str {
        match self {
            Status::Continue => "Continue",
            Status::SwitchingProtocols => "Switching Protocols",
            Status::EarlyHints => "Early Hints",
            Status::Ok => "OK",
            Status::Created => "Created",
            Status::Accepted => "Accepted",
            Status::NonAuthoritativeInformation => "Non Authoritative Information",
            Status::NoContent => "No Content",
            Status::ResetContent => "Reset Content",
            Status::PartialContent => "Partial Content",
            Status::MultiStatus => "Multi-Status",
            Status::ImUsed => "Im Used",
            Status::MultipleChoice => "Multiple Choice",
            Status::MovedPermanently => "Moved Permanently",
            Status::Found => "Found",
            Status::SeeOther => "See Other",
            Status::NotModified => "Not Modified",
            Status::TemporaryRedirect => "Temporary Redirect",
            Status::PermanentRedirect => "Permanent Redirect",
            Status::BadRequest => "Bad Request",
            Status::Unauthorized => "Unauthorized",
            Status::PaymentRequired => "Payment Required",
            Status::Forbidden => "Forbidden",
            Status::NotFound => "Not Found",
            Status::MethodNotAllowed => "Method Not Allowed",
            Status::NotAcceptable => "Not Acceptable",
            Status::ProxyAuthenticationRequired => "Proxy Authentication Required",
            Status::RequestTimeout => "Request Timeout",
            Status::Conflict => "Conflict",
            Status::Gone => "Gone",
            Status::LengthRequired => "Length Required",
            Status::PreconditionFailed => "Precondition Failed",
            Status::PayloadTooLarge => "Payload Too Large",
            Status::UriTooLong => "URI Too Long",
            Status::UnsupportedMediaType => "Unsupported Media Type",
            Status::RequestedRangeNotSatisfiable => "Requested Range Not Satisfiable",
            Status::ExpectationFailed => "Expectation Failed",
            Status::ImATeapot => "I'm a teapot",
            Status::MisdirectedRequest => "Misdirected Request",
            Status::UnprocessableEntity => "Unprocessable Entity",
            Status::Locked => "Locked",
            Status::FailedDependency => "Failed Dependency",
            Status::TooEarly => "Too Early",
            Status::UpgradeRequired => "Upgrade Required",
            Status::PreconditionRequired => "Precondition Required",
            Status::TooManyRequests => "Too Many Requests",
            Status::RequestHeaderFieldsTooLarge => "Request Header Fields Too Large",
            Status::UnavailableForLegalReasons => "Unavailable For Legal Reasons",
            Status::InternalServerError => "Internal Server Error",
            Status::NotImplemented => "Not Implemented",
            Status::BadGateway => "Bad Gateway",
            Status::ServiceUnavailable => "Service Unavailable",
            Status::GatewayTimeout => "Gateway Timeout",
            Status::HttpVersionNotSupported => "HTTP Version Not Supported",
            Status::VariantAlsoNegotiates => "Variant Also Negotiates",
            Status::InsufficientStorage => "Insufficient Storage",
            Status::LoopDetected => "Loop Detected",
            Status::NotExtended => "Not Extended",
            Status::NetworkAuthenticationRequired => "Network Authentication Required",
        }
    }
}

impl From<Status> for u16 {
    fn from(code: Status) -> u16 {
        code as u16
    }
}

impl TryFrom<u16> for Status {
    type Error = Error;

    fn try_from(num: u16) -> Result<Self, Self::Error> {
        match num {
            100 => Ok(Status::Continue),
            101 => Ok(Status::SwitchingProtocols),
            103 => Ok(Status::EarlyHints),
            200 => Ok(Status::Ok),
            201 => Ok(Status::Created),
            202 => Ok(Status::Accepted),
            203 => Ok(Status::NonAuthoritativeInformation),
            204 => Ok(Status::NoContent),
            205 => Ok(Status::ResetContent),
            206 => Ok(Status::PartialContent),
            207 => Ok(Status::MultiStatus),
            226 => Ok(Status::ImUsed),
            300 => Ok(Status::MultipleChoice),
            301 => Ok(Status::MovedPermanently),
            302 => Ok(Status::Found),
            303 => Ok(Status::SeeOther),
            304 => Ok(Status::NotModified),
            307 => Ok(Status::TemporaryRedirect),
            308 => Ok(Status::PermanentRedirect),
            400 => Ok(Status::BadRequest),
            401 => Ok(Status::Unauthorized),
            402 => Ok(Status::PaymentRequired),
            403 => Ok(Status::Forbidden),
            404 => Ok(Status::NotFound),
            405 => Ok(Status::MethodNotAllowed),
            406 => Ok(Status::NotAcceptable),
            407 => Ok(Status::ProxyAuthenticationRequired),
            408 => Ok(Status::RequestTimeout),
            409 => Ok(Status::Conflict),
            410 => Ok(Status::Gone),
            411 => Ok(Status::LengthRequired),
            412 => Ok(Status::PreconditionFailed),
            413 => Ok(Status::PayloadTooLarge),
            414 => Ok(Status::UriTooLong),
            415 => Ok(Status::UnsupportedMediaType),
            416 => Ok(Status::RequestedRangeNotSatisfiable),
            417 => Ok(Status::ExpectationFailed),
            418 => Ok(Status::ImATeapot),
            421 => Ok(Status::MisdirectedRequest),
            422 => Ok(Status::UnprocessableEntity),
            423 => Ok(Status::Locked),
            424 => Ok(Status::FailedDependency),
            425 => Ok(Status::TooEarly),
            426 => Ok(Status::UpgradeRequired),
            428 => Ok(Status::PreconditionRequired),
            429 => Ok(Status::TooManyRequests),
            431 => Ok(Status::RequestHeaderFieldsTooLarge),
            451 => Ok(Status::UnavailableForLegalReasons),
            500 => Ok(Status::InternalServerError),
            501 => Ok(Status::NotImplemented),
            502 => Ok(Status::BadGateway),
            503 => Ok(Status::ServiceUnavailable),
            504 => Ok(Status::GatewayTimeout),
            505 => Ok(Status::HttpVersionNotSupported),
            506 => Ok(Status::VariantAlsoNegotiates),
            507 => Ok(Status::InsufficientStorage),
            508 => Ok(Status::LoopDetected),
            510 => Ok(Status::NotExtended),
            511 => Ok(Status::NetworkAuthenticationRequired),
            _ => Err(Error::InvalidStatus),
        }
    }
}

impl PartialEq<Status> for u16 {
    fn eq(&self, other: &Status) -> bool {
        *self == *other as u16
    }
}

impl PartialEq<u16> for Status {
    fn eq(&self, other: &u16) -> bool {
        *self as u16 == *other
    }
}

impl Debug for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Debug::fmt(&(*self as u16), f)
    }
}

impl Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", *self as u16, self.canonical_reason())
    }
}

impl FromStr for Status {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        u16::from_str(s)
            .map_err(|_| Error::InvalidStatus)?
            .try_into()
    }
}
