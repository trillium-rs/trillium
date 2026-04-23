//! Trillium HTTP/2 types (RFC 9113).
//!
//! This module is the server-side HTTP/2 implementation used by `trillium-http`. Most items are
//! crate-private; only the error types are currently part of the public surface.

mod error;
mod settings;

pub use error::H2ErrorCode;
pub use settings::H2Settings;

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
