//! Trillium HTTP/2 types (RFC 9113).
//!
//! This module is the server-side HTTP/2 implementation used by `trillium-http`. Most items are
//! crate-private; only the error types are part of the public surface.
mod acceptor;
mod body_wrapper;
mod connection;
mod error;
mod frame;
#[cfg(feature = "unstable")]
mod initiator;
mod role;
mod settings;
mod transport;

use crate::headers::compression_error::CompressionError;
pub use acceptor::H2Driver;
pub(crate) use body_wrapper::H2Body;
pub use connection::H2Connection;
#[cfg(feature = "unstable")]
#[doc(hidden)]
pub use connection::{ResponseHeaders, SubmitSend};
pub use error::H2ErrorCode;
#[cfg(feature = "unstable")]
pub use initiator::H2Initiator;
#[cfg(feature = "unstable")]
pub use settings::H2Settings;
#[cfg(not(feature = "unstable"))]
pub(crate) use settings::H2Settings;
pub use transport::H2Transport;

/// An error that may occur during HTTP/2 stream or connection processing.
///
/// When the error is `Protocol`, the contained [`H2ErrorCode`] should be communicated to the peer
/// via GOAWAY (for connection-level errors) or `RST_STREAM` (for stream-level errors); the
/// distinction is contextual. `Io` errors indicate an unrecoverable transport failure.
#[derive(thiserror::Error, Debug)]
pub enum H2Error {
    /// An HTTP/2 protocol error; the code should be signalled to the peer.
    #[error(transparent)]
    Protocol(#[from] H2ErrorCode),

    /// An unrecoverable I/O error encountered at the network layer.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// HPACK decoding failures map to `COMPRESSION_ERROR`.
impl From<CompressionError> for H2Error {
    fn from(_: CompressionError) -> Self {
        Self::Protocol(H2ErrorCode::CompressionError)
    }
}
