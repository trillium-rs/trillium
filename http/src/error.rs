use std::borrow::Cow;
use std::num::TryFromIntError;
use std::str::Utf8Error;

use thiserror::Error;

use crate::Version;

/// Concrete errors that occur within trillium's http implementation
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    /// [`std::io::Error`]
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// this error describes a malformed request with a path that does
    /// not start with / or http:// or https://
    #[error("unexpected uri format")]
    UnexpectedUriFormat,

    /// the relevant http protocol expected this header, but it was
    /// not provided
    #[error("mandatory {0} header missing")]
    HeaderMissing(&'static str),

    /// this error describes a request that does not specify a path
    #[error("request path missing")]
    RequestPathMissing,

    /// connection was closed
    #[error("connection closed by client")]
    Closed,

    /// [`httparse::Error`]
    #[error(transparent)]
    Httparse(#[from] httparse::Error),

    /// [`TryFromIntError`]
    #[error(transparent)]
    TryFromIntError(#[from] TryFromIntError),

    /// an incomplete http head
    #[error("partial http head")]
    PartialHead,

    /// we were unable to parse a header
    #[error("malformed http header {0}")]
    MalformedHeader(Cow<'static, str>),

    /// trillium doesn't speak this http version
    /// this error is deprecated
    #[deprecated = "trillium will only return RecognizedBufUnsupportedVersion now"]
    #[error("unsupported http version 1.{0}")]
    UnsupportedVersion(u8),

    /// trillium recognizes this http version but does not support it
    #[error("unsupported http version {0}")]
    RecognizedButUnsupportedVersion(Version),

    /// trillium cannot parse this http version
    #[error("unrecognized version {0}")]
    UnrecognizedVersion(String),

    /// we were unable to parse this http method
    #[error("unsupported or unrecognized http method {0}")]
    UnrecognizedMethod(String),

    /// this request did not have a method
    #[error("missing method")]
    MissingMethod,

    /// this request did not have a status code
    #[error("missing status code")]
    MissingStatusCode,

    /// we were unable to parse this http method
    #[error("unrecognized http status code {0}")]
    UnrecognizedStatusCode(u16),

    /// this request did not have a version, but we expect one
    /// this error is deprecated
    #[error("missing version")]
    MissingVersion,

    /// we expected utf8, but there was an encoding error
    #[error(transparent)]
    EncodingError(#[from] Utf8Error),

    /// we received a header that does not make sense in context
    #[error("unexpected header: {0}")]
    UnexpectedHeader(&'static str),

    /// to mitigate against malicious http clients, we do not allow request headers beyond this
    /// length.
    #[error("Headers were malformed or longer than allowed")]
    HeadersTooLong,

    /// to mitigate against malicious http clients, we do not read received bodies beyond this
    /// length to memory. If you need to receive longer bodies, use the Stream or AsyncRead
    /// implementation on ReceivedBody
    #[error("Received body too long. Maximum {0} bytes")]
    ReceivedBodyTooLong(u64),

    /// Something went wrong with header parsing
    #[error("Malformed header")]
    MalformedHeaders,
}

/// this crate's result type
pub type Result<T> = std::result::Result<T, Error>;
