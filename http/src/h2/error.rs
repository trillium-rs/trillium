use std::fmt;

/// H2 error codes per RFC 9113 §7.
///
/// The same codes appear in both GOAWAY (connection errors) and `RST_STREAM` (stream errors);
/// whether a given use is connection- or stream-level is determined by context, not by the code
/// itself. Unknown wire values decode to [`Self::NoError`] per §5.4.4 / §5.4.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H2ErrorCode {
    /// Graceful shutdown or no error to signal.
    NoError = 0x0,

    /// Peer violated protocol requirements.
    ProtocolError = 0x1,

    /// An internal error in the HTTP stack.
    InternalError = 0x2,

    /// Peer violated flow-control limits.
    FlowControlError = 0x3,

    /// Settings frame was not acknowledged in a timely manner.
    SettingsTimeout = 0x4,

    /// A frame was received on a closed stream.
    StreamClosed = 0x5,

    /// A frame of an incorrect size was received.
    FrameSizeError = 0x6,

    /// The stream was refused before any application processing.
    RefusedStream = 0x7,

    /// The stream was cancelled.
    Cancel = 0x8,

    /// HPACK compression state could not be maintained.
    CompressionError = 0x9,

    /// TCP connection for a CONNECT request was reset or abnormally closed.
    ConnectError = 0xa,

    /// Peer is generating excessive load.
    EnhanceYourCalm = 0xb,

    /// Negotiated TLS parameters are unacceptable.
    InadequateSecurity = 0xc,

    /// Request must be retried over HTTP/1.1.
    Http1_1Required = 0xd,
}

impl H2ErrorCode {
    /// A reason phrase suitable for GOAWAY debug data.
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::NoError => "Graceful shutdown or no error to signal.",
            Self::ProtocolError => "Peer violated protocol requirements.",
            Self::InternalError => "An internal error in the HTTP stack.",
            Self::FlowControlError => "Peer violated flow-control limits.",
            Self::SettingsTimeout => "Settings frame was not acknowledged in a timely manner.",
            Self::StreamClosed => "A frame was received on a closed stream.",
            Self::FrameSizeError => "A frame of an incorrect size was received.",
            Self::RefusedStream => "The stream was refused before any application processing.",
            Self::Cancel => "The stream was cancelled.",
            Self::CompressionError => "HPACK compression state could not be maintained.",
            Self::ConnectError => {
                "TCP connection for a CONNECT request was reset or abnormally closed."
            }
            Self::EnhanceYourCalm => "Peer is generating excessive load.",
            Self::InadequateSecurity => "Negotiated TLS parameters are unacceptable.",
            Self::Http1_1Required => "Request must be retried over HTTP/1.1.",
        }
    }
}

impl fmt::Display for H2ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str((*self).reason())
    }
}

impl std::error::Error for H2ErrorCode {}

impl From<u32> for H2ErrorCode {
    /// Unknown error codes decode to [`Self::NoError`] per RFC 9113 §5.4.4 / §5.4.5.
    fn from(value: u32) -> Self {
        match value {
            0x1 => Self::ProtocolError,
            0x2 => Self::InternalError,
            0x3 => Self::FlowControlError,
            0x4 => Self::SettingsTimeout,
            0x5 => Self::StreamClosed,
            0x6 => Self::FrameSizeError,
            0x7 => Self::RefusedStream,
            0x8 => Self::Cancel,
            0x9 => Self::CompressionError,
            0xa => Self::ConnectError,
            0xb => Self::EnhanceYourCalm,
            0xc => Self::InadequateSecurity,
            0xd => Self::Http1_1Required,
            _ => Self::NoError,
        }
    }
}

impl From<H2ErrorCode> for u32 {
    fn from(code: H2ErrorCode) -> u32 {
        code as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_roundtrip() {
        for code in [
            H2ErrorCode::NoError,
            H2ErrorCode::ProtocolError,
            H2ErrorCode::InternalError,
            H2ErrorCode::FlowControlError,
            H2ErrorCode::SettingsTimeout,
            H2ErrorCode::StreamClosed,
            H2ErrorCode::FrameSizeError,
            H2ErrorCode::RefusedStream,
            H2ErrorCode::Cancel,
            H2ErrorCode::CompressionError,
            H2ErrorCode::ConnectError,
            H2ErrorCode::EnhanceYourCalm,
            H2ErrorCode::InadequateSecurity,
            H2ErrorCode::Http1_1Required,
        ] {
            let wire: u32 = code.into();
            assert_eq!(
                H2ErrorCode::from(wire),
                code,
                "roundtrip failed for {code:?}"
            );
        }
    }

    #[test]
    fn unknown_codes_decode_as_no_error() {
        assert_eq!(H2ErrorCode::from(0xdead_beef), H2ErrorCode::NoError);
        assert_eq!(H2ErrorCode::from(0xe), H2ErrorCode::NoError);
        assert_eq!(H2ErrorCode::from(u32::MAX), H2ErrorCode::NoError);
    }
}
