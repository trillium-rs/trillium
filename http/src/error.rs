use crate::{HeaderName, Version};
use std::{num::TryFromIntError, str::Utf8Error};
use thiserror::Error;

/// Concrete errors that occur within trillium's HTTP implementation
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum Error {
    /// [`std::io::Error`]
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// this error describes a malformed request with a path that does
    /// not start with / or http:// or https://
    #[error("Unexpected uri format")]
    UnexpectedUriFormat,

    /// the relevant HTTP protocol expected this header, but it was
    /// not provided
    #[error("Mandatory {0} header missing")]
    HeaderMissing(HeaderName<'static>),

    /// this error describes a request that does not specify a path
    #[error("Request path missing")]
    RequestPathMissing,

    /// connection was closed
    #[error("Connection closed by client")]
    Closed,

    /// [`TryFromIntError`]
    #[error(transparent)]
    TryFromIntError(#[from] TryFromIntError),

    /// An incomplete or invalid HTTP head
    #[error("Partial or invalid HTTP head")]
    InvalidHead,

    /// We were unable to parse a [`HeaderName`][crate::HeaderName]
    #[error("Invalid or unparseable header name")]
    InvalidHeaderName,

    /// We were unable to parse a [`HeaderValue`][crate::HeaderValue]
    #[error("Invalid or unparseable header value, header name: {0}")]
    InvalidHeaderValue(HeaderName<'static>),

    /// we were able to parse this [`Version`], but we do not support it
    #[error("Unsupported version {0}")]
    UnsupportedVersion(Version),

    /// We were unable to parse a [`Version`]
    #[error("Invalid or missing version")]
    InvalidVersion,

    /// we were unable to parse this method
    #[error("Unsupported method {0}")]
    UnrecognizedMethod(String),

    /// this request did not have a method
    #[error("Missing method")]
    MissingMethod,

    /// this request did not have a status code
    #[error("Missing status code")]
    MissingStatus,

    /// we were unable to parse a [`Status`]
    #[error("Invalid status code")]
    InvalidStatus,

    /// we expected utf8, but there was an encoding error
    #[error(transparent)]
    EncodingError(#[from] Utf8Error),

    /// we either received a header that does not make sense in context
    #[error("Unexpected header: {0}")]
    UnexpectedHeader(HeaderName<'static>),

    /// to mitigate against malicious HTTP clients, we do not allow request headers beyond this
    /// length.
    #[error("Headers were malformed or longer than allowed")]
    HeadersTooLong,

    /// to mitigate against malicious HTTP clients, we do not read received bodies beyond this
    /// length to memory. If you need to receive longer bodies, use the Stream or `AsyncRead`
    /// implementation on `ReceivedBody`
    #[error("Received body too long. Maximum {0} bytes")]
    ReceivedBodyTooLong(u64),
}

/// this crate's result type
pub type Result<T> = std::result::Result<T, Error>;
