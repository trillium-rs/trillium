//! Trillium HTTP/3 types

mod body_wrapper;
mod connection;
mod error;
mod frame;
#[cfg(feature = "unstable")]
pub mod quic_varint;
#[cfg(not(feature = "unstable"))]
mod quic_varint;
mod settings;

#[cfg(test)]
mod tests;

/// An error that may occur during HTTP/3 stream or connection processing.
///
/// When the error is `Protocol`, the contained [`H3ErrorCode`] should be communicated to the
/// peer via the QUIC connection's error signaling. `Io` errors indicate an unrecoverable
/// transport failure.
#[derive(thiserror::Error, Debug)]
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
#[cfg(not(feature = "unstable"))]
pub(crate) use body_wrapper::H3Body;
pub use connection::{H3Connection, H3StreamResult, UniStreamResult};
pub use error::H3ErrorCode;
pub(crate) use frame::UniStreamType;
#[cfg(feature = "unstable")]
pub use frame::{ActiveFrame, Frame, FrameDecodeError, FrameStream};
#[cfg(not(feature = "unstable"))]
pub(crate) use frame::{Frame, FrameDecodeError, FrameStream};
