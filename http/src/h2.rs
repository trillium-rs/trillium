//! Trillium HTTP/2 types (RFC 9113).
//!
//! This module is the server-side HTTP/2 implementation used by `trillium-http`. Most items are
//! crate-private; only the error types are currently part of the public surface.
//!
//! The module-wide `dead_code` allowance is intentional while the connection driver is still
//! being built — the per-frame encoders are used by unit tests but have no production caller yet.
#![allow(dead_code)]

mod connection;
mod error;
mod frame;
mod settings;
mod transport;

pub use connection::{H2Acceptor, H2Connection, TransportPlaceholder};
pub use error::H2ErrorCode;
pub use frame::{Frame, FrameDecodeError, PriorityInfo};
pub use settings::H2Settings;
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
