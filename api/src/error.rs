use crate::ApiConnExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fmt::Display;
use trillium::{async_trait, Conn, Handler, Status};

/// A serde-serializable error
#[derive(Serialize, Deserialize, Debug, Clone, thiserror::Error)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Error {
    /// An error occurred in parsing the provided body content
    #[error("Parse error at {path}: {message}")]
    ParseError {
        /// the path of the parse error, as provided by [`serde_path_to_error`]
        path: String,
        /// the contents of the error
        message: String,
    },
    /// A transmission error occurred in the connection to the http
    /// client
    #[error("I/O error type {kind}: {message}")]
    IoError {
        /// stringified [`std::io::ErrorKind`]
        kind: String,
        /// stringified [`std::io::Error`]
        message: String,
    },
    /// The client provided a content type that this library does not
    /// yet support
    #[error("Unsupported mime type: {mime_type}")]
    UnsupportedMimeType {
        /// the unsupported mime type
        mime_type: String,
    },
    /// The client did not provide a content-type
    #[error("Missing content type")]
    MissingContentType,
    /// Miscellaneous other errors -- please open an issue on
    /// trillium-api if you find yourself parsing the contents of
    /// this.
    #[error("{message}")]
    Other {
        /// A stringified error
        message: String,
    },

    #[error("No negotiated mime type")]
    /// we were unable to find a content type that matches the Accept
    /// header. Please open an issue if you'd like an additional
    /// format to be supported
    FailureToNegotiateContent,
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::ParseError {
            path: format!("{}:{}", value.line(), value.column()),
            message: value.to_string(),
        }
    }
}

impl From<trillium::Error> for Error {
    fn from(error: trillium::Error) -> Self {
        match error {
            trillium::Error::Io(e) => Self::IoError {
                kind: e.kind().to_string(),
                message: e.to_string(),
            },

            other => Self::Other {
                message: other.to_string(),
            },
        }
    }
}

impl<E: Display> From<serde_path_to_error::Error<E>> for Error {
    fn from(e: serde_path_to_error::Error<E>) -> Self {
        Error::ParseError {
            path: e.path().to_string(),
            message: e.to_string(),
        }
    }
}

#[cfg(feature = "forms")]
impl From<serde_urlencoded::ser::Error> for Error {
    fn from(value: serde_urlencoded::ser::Error) -> Self {
        Error::Other {
            message: value.to_string(),
        }
    }
}
#[cfg(feature = "forms")]
impl From<serde_urlencoded::de::Error> for Error {
    fn from(value: serde_urlencoded::de::Error) -> Self {
        Error::ParseError {
            path: "".into(),
            message: value.to_string(),
        }
    }
}

#[async_trait]
impl Handler for Error {
    async fn run(&self, conn: Conn) -> Conn {
        conn.with_state(self.clone()).halt()
    }

    async fn before_send(&self, mut conn: Conn) -> Conn {
        if let Some(error) = conn.take_state::<Self>() {
            conn.with_json(&json!({ "error": &error }))
                .with_status(&error)
        } else {
            conn
        }
    }
}

impl From<&Error> for Status {
    fn from(value: &Error) -> Self {
        match value {
            Error::ParseError { .. } => Status::UnprocessableEntity,
            Error::UnsupportedMimeType { .. } | Error::MissingContentType => {
                Status::UnsupportedMediaType
            }
            Error::FailureToNegotiateContent => Status::NotAcceptable,
            _ => Status::InternalServerError,
        }
    }
}

#[async_trait]
impl crate::FromConn for Error {
    async fn from_conn(conn: &mut Conn) -> Option<Self> {
        conn.take_state()
    }
}
