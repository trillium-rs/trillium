//! Huffman coding for header strings, per RFC 7541.

mod decode;
mod encode;
mod table;
#[cfg(test)]
mod tests;

pub(in crate::headers) use decode::decode;
pub(in crate::headers) use encode::{encode_into, encoded_length_if_shorter};
use table::TABLE;

/// Errors that can occur during Huffman decoding.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone, Copy)]
#[non_exhaustive]
pub enum HuffmanError {
    /// The encoded data contained the EOS symbol, which must not appear in the
    /// encoded stream.
    #[error("EOS symbol encountered in encoded data")]
    EosInStream,

    /// Padding bits were not all ones.
    #[error("invalid padding (not all ones)")]
    InvalidPadding,

    /// Padding was longer than 7 bits, indicating a full unused byte which is not
    /// permitted.
    #[error("padding too long (more than 7 bits)")]
    PaddingTooLong,
}
