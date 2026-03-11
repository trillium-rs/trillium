mod decode;
mod encode;
mod table;
#[cfg(test)]
mod tests;

pub(crate) use decode::decode;
pub(crate) use encode::encode;
use table::TABLE;

/// Errors that can occur during Huffman decoding.
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone, Copy)]
pub enum HuffmanError {
    /// The encoded data contained the EOS symbol, which must not
    /// appear in the encoded stream (RFC 7541 §5.2).
    #[error("EOS symbol encountered in encoded data")]
    EosInStream,

    /// Padding bits were not all ones, as required by RFC 7541 §5.2.
    #[error("invalid padding (not all ones)")]
    InvalidPadding,

    /// Padding was longer than 7 bits, indicating a full unused byte
    /// which is not permitted (RFC 7541 §5.2).
    #[error("padding too long (more than 7 bits)")]
    PaddingTooLong,
}
