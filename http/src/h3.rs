//! Trillium HTTP/3 types

// Retained only so an older `trillium-client` that predates `Body::write_into` still builds
// against this crate; remove it at the next breaking release.
#[cfg(feature = "unstable")]
mod body_wrapper;
mod connection;
mod error;
mod frame;
#[cfg(feature = "unstable")]
pub mod quic_varint;
#[cfg(not(feature = "unstable"))]
pub(crate) mod quic_varint;
mod settings;

#[cfg(all(test, feature = "unstable"))]
mod tests;

/// An error that may occur during HTTP/3 stream or connection processing.
///
/// When the error is `Protocol`, the contained [`H3ErrorCode`] should be communicated to the
/// peer via the QUIC connection's error signaling. `Io` errors indicate an unrecoverable
/// transport failure.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum H3Error {
    #[error(transparent)]
    /// An HTTP/3 protocol error; the error code should be signaled to the peer.
    Protocol(#[from] H3ErrorCode),

    #[error(transparent)]
    /// An unrecoverable I/O error encountered at the network layer.
    Io(#[from] std::io::Error),
}

#[cfg(feature = "unstable")]
pub use body_wrapper::H3Body;
pub use connection::{H3BidiRequest, H3Connection, H3StreamResult, UniStreamResult};
pub use error::H3ErrorCode;
pub(crate) use frame::UniStreamType;
#[cfg(feature = "unstable")]
pub use frame::{ActiveFrame, Frame, FrameDecodeError, FrameStream};
#[cfg(not(feature = "unstable"))]
pub(crate) use frame::{Frame, FrameDecodeError, FrameStream};
pub(crate) use settings::H3Settings;

pub(crate) const MAX_BUFFER_SIZE: usize = 1024 * 10;
