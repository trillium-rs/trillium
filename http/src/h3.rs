//! Trillium HTTP/3 types

mod body_wrapper;
mod connection;
mod error;
mod frame;
mod quic_varint;
mod settings;

#[cfg(test)]
mod tests;

/// Error type that bears a transport. Callers at the QUIC protocol layer are expected to combine
/// the `ErrorCode` and the Transport in order to send the error to the peer. I/O errors do not
/// currently contain a transport for return because we assume they're terminal.
#[derive(thiserror::Error, Debug)]
pub enum H3RequestError {
    #[error(transparent)]
    /// HTTP/3 Protocol error to be communicated on the attached Transport
    Protocol(#[from] ErrorCode),

    #[error(transparent)]
    /// An unrecoverable I/O error encountered at the network layer
    Io(#[from] std::io::Error),
}

pub(crate) use body_wrapper::H3BodyWrapper;
pub use connection::H3Connection;
pub use error::ErrorCode;
pub(crate) use frame::{Frame, FrameDecodeError, FrameStream};
