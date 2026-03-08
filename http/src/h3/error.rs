use std::borrow::Cow;

/// H3 error codes per RFC 9114 §8.1.
///
/// Used when closing connections or resetting streams.
/// Unknown error codes are mapped to `NoError` per spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ErrorCode {
    /// No error. Used when closing without an error to signal.
    #[error("No error. Used when closing without an error to signal.")]
    NoError = 0x0100,

    /// Peer violated protocol requirements.
    #[error("Peer violated protocol requirements.")]
    GeneralProtocolError = 0x0101,

    /// An internal error in the HTTP stack.
    #[error("An internal error in the HTTP stack.")]
    InternalError = 0x0102,

    /// Peer created a stream that will not be accepted.
    #[error("Peer created a stream that will not be accepted.")]
    StreamCreationError = 0x0103,

    /// A required stream was closed or reset.
    #[error("A required stream was closed or reset.")]
    ClosedCriticalStream = 0x0104,

    /// A frame was not permitted in the current state or stream.
    #[error("A frame was not permitted in the current state or stream.")]
    FrameUnexpected = 0x0105,

    /// A frame fails layout requirements or has an invalid size.
    #[error("A frame fails layout requirements or has an invalid size.")]
    FrameError = 0x0106,

    /// Peer is generating excessive load.
    #[error("Peer is generating excessive load.")]
    ExcessiveLoad = 0x0107,

    /// A stream ID or push ID was used incorrectly.
    #[error("A stream ID or push ID was used incorrectly.")]
    IdError = 0x0108,

    /// Error in the payload of a SETTINGS frame.
    #[error("Error in the payload of a SETTINGS frame.")]
    SettingsError = 0x0109,

    /// No SETTINGS frame at the beginning of the control stream.
    #[error("No SETTINGS frame at the beginning of the control stream.")]
    MissingSettings = 0x010a,

    /// Server rejected a request without application processing.
    #[error("Server rejected a request without application processing.")]
    RequestRejected = 0x010b,

    /// Request or response (including pushed) is cancelled.
    #[error("Request or response (including pushed) is cancelled.")]
    RequestCancelled = 0x010c,

    /// Client stream terminated without a fully formed request.
    #[error("Client stream terminated without a fully formed request.")]
    RequestIncomplete = 0x010d,

    /// HTTP message was malformed.
    #[error("HTTP message was malformed.")]
    MessageError = 0x010e,

    /// TCP connection for CONNECT was reset or abnormally closed.
    #[error("TCP connection for CONNECT was reset or abnormally closed.")]
    ConnectError = 0x010f,

    /// Requested operation cannot be served over HTTP/3.
    #[error("Requested operation cannot be served over HTTP/3.")]
    VersionFallback = 0x0110,
}

impl ErrorCode {
    /// A "reason phrase" per rfc9000 §19.19
    pub fn reason(&self) -> Cow<'static, str> {
        // eventually this probably should either be &'static str or callsite-specific
        Cow::Owned(format!("{self}"))
    }
}

impl From<u64> for ErrorCode {
    /// All unknown error codes are treated as equivalent to `NoError`
    /// per RFC 9114 §9.
    fn from(value: u64) -> Self {
        match value {
            0x0101 => Self::GeneralProtocolError,
            0x0102 => Self::InternalError,
            0x0103 => Self::StreamCreationError,
            0x0104 => Self::ClosedCriticalStream,
            0x0105 => Self::FrameUnexpected,
            0x0106 => Self::FrameError,
            0x0107 => Self::ExcessiveLoad,
            0x0108 => Self::IdError,
            0x0109 => Self::SettingsError,
            0x010a => Self::MissingSettings,
            0x010b => Self::RequestRejected,
            0x010c => Self::RequestCancelled,
            0x010d => Self::RequestIncomplete,
            0x010e => Self::MessageError,
            0x010f => Self::ConnectError,
            0x0110 => Self::VersionFallback,
            _ => Self::NoError,
        }
    }
}

impl From<ErrorCode> for u64 {
    /// Encodes the error code. `NoError` emits a random GREASE value
    /// (`0x1f * N + 0x21`) per RFC 9114 §8.1 to exercise peer handling
    /// of unknown codes.
    fn from(code: ErrorCode) -> u64 {
        match code {
            ErrorCode::NoError => {
                let n = u64::from(fastrand::u16(..));
                0x1f * n + 0x21
            }
            other => other as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_roundtrip() {
        for code in [
            ErrorCode::GeneralProtocolError,
            ErrorCode::InternalError,
            ErrorCode::StreamCreationError,
            ErrorCode::ClosedCriticalStream,
            ErrorCode::FrameUnexpected,
            ErrorCode::FrameError,
            ErrorCode::ExcessiveLoad,
            ErrorCode::IdError,
            ErrorCode::SettingsError,
            ErrorCode::MissingSettings,
            ErrorCode::RequestRejected,
            ErrorCode::RequestCancelled,
            ErrorCode::RequestIncomplete,
            ErrorCode::MessageError,
            ErrorCode::ConnectError,
            ErrorCode::VersionFallback,
        ] {
            let wire: u64 = code.into();
            let decoded = ErrorCode::from(wire);
            assert_eq!(decoded, code, "roundtrip failed for {code:?}");
        }
    }

    #[test]
    fn no_error_encodes_as_grease() {
        for _ in 0..100 {
            let wire: u64 = ErrorCode::NoError.into();
            assert_ne!(wire, 0x0100, "should emit GREASE, not literal NoError");
            assert_eq!(
                (wire - 0x21) % 0x1f,
                0,
                "{wire:#x} is not a valid GREASE value"
            );
        }
    }

    #[test]
    fn grease_decodes_as_no_error() {
        for n in [0u64, 1, 100, 0xFFFF] {
            let grease = 0x1f * n + 0x21;
            assert_eq!(ErrorCode::from(grease), ErrorCode::NoError);
        }
    }

    #[test]
    fn unknown_non_grease_decodes_as_no_error() {
        assert_eq!(ErrorCode::from(0xDEAD), ErrorCode::NoError);
        assert_eq!(ErrorCode::from(0), ErrorCode::NoError);
        assert_eq!(ErrorCode::from(u64::MAX), ErrorCode::NoError);
    }
}
