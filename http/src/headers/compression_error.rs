//! Shared error type for HPACK/QPACK header compression (RFC 7541 + RFC 9204).
//!
//! Both protocols decode into a near-identical error vocabulary: Huffman malformed input,
//! integer-prefix decode failure, static-table index out of range, unexpected end of input,
//! and invalid field name. The enum is crate-private; each protocol maps it to its own
//! wire-level connection-error code via a `From` impl.

use super::{huffman::HuffmanError, integer_prefix::IntegerPrefixError};
use crate::h3::{H3Error, H3ErrorCode};

/// Errors produced by the header-compression codecs (HPACK and QPACK).
#[derive(Debug, thiserror::Error, Clone, Copy)]
pub(crate) enum CompressionError {
    #[error(transparent)]
    Huffman(#[from] HuffmanError),

    #[error(transparent)]
    IntegerPrefix(#[from] IntegerPrefixError),

    #[error("static table index {0} out of range")]
    InvalidStaticIndex(usize),

    #[error("unexpected end of field section")]
    UnexpectedEnd,

    #[error("invalid header name")]
    InvalidHeaderName,
}

impl From<CompressionError> for H3Error {
    /// Most `CompressionError` variants are codec failures the QPACK layer must report as
    /// `QPACK_DECOMPRESSION_FAILED` (RFC 9204 §6). `InvalidHeaderName` is the exception:
    /// per RFC 9114 §4.2 / §4.3.1 a header name with uppercase chars or an unrecognized
    /// pseudo-name is a *malformed-message* problem (the codec succeeded; the resulting
    /// message is invalid), which is a stream-level `H3_MESSAGE_ERROR`.
    fn from(error: CompressionError) -> Self {
        match error {
            CompressionError::InvalidHeaderName => H3ErrorCode::MessageError.into(),
            CompressionError::Huffman(_)
            | CompressionError::IntegerPrefix(_)
            | CompressionError::InvalidStaticIndex(_)
            | CompressionError::UnexpectedEnd => H3ErrorCode::QpackDecompressionFailed.into(),
        }
    }
}
