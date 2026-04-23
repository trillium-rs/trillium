//! QPACK types
//!
//! Please note that this interface is likely to change
#[cfg(test)]
mod decoder_corpus_tests;
mod decoder_dynamic_table;
#[cfg(test)]
mod encoder_corpus_tests;
mod encoder_dynamic_table;
mod header_observer;
pub(crate) mod huffman;
mod instruction;
#[cfg(test)]
mod qif;
#[cfg(test)]
mod reference_out;
pub(crate) mod static_table;
#[cfg(test)]
mod tests;
pub(crate) mod varint;

// Wire-format constants for §4.5 field sections live in `instruction::field_section`.
// Encoder-stream instruction constants (§3.2) live in `instruction::encoder`.
// Decoder-stream instruction constants (§4.4) live in `instruction::decoder`.
// §4.1.2 string-literal encoding helpers live in `instruction` (module-level).
// `FieldSection`, `PseudoHeaders`, and `FieldLineValue` live at `headers::field_section` so
// that HPACK can share them without depending on QPACK. Re-exported here so existing
// `qpack::{FieldSection, PseudoHeaders}` callsites and the feature-gated public API
// continue to work.
pub(crate) use super::field_section::FieldLineValue;
pub use super::field_section::{FieldSection, PseudoHeaders};
use crate::{
    h3::{H3Error, H3ErrorCode},
    headers::qpack::varint::VarIntError,
};
#[cfg(not(feature = "unstable"))]
pub(crate) use decoder_dynamic_table::DecoderDynamicTable;
#[cfg(feature = "unstable")]
pub use decoder_dynamic_table::DecoderDynamicTable;
#[cfg(not(feature = "unstable"))]
pub(crate) use encoder_dynamic_table::EncoderDynamicTable;
#[cfg(feature = "unstable")]
pub use encoder_dynamic_table::EncoderDynamicTable;
pub(crate) use header_observer::HeaderObserver;
#[cfg(feature = "unstable")]
pub use huffman::HuffmanError;
#[cfg(not(feature = "unstable"))]
use huffman::HuffmanError;

/// Errors that can occur during QPACK decoding.
#[derive(Debug, thiserror::Error, Clone, Copy)]
pub(crate) enum QpackError {
    #[error(transparent)]
    Huffman(#[from] HuffmanError),

    #[error(transparent)]
    VarInt(#[from] VarIntError),

    #[error("static table index {0} out of range (0-98)")]
    InvalidStaticIndex(usize),

    #[error("unexpected end of field section")]
    UnexpectedEnd,

    #[error("invalid header name")]
    InvalidHeaderName,
}

impl From<QpackError> for H3Error {
    fn from(_: QpackError) -> Self {
        H3ErrorCode::QpackDecompressionFailed.into()
    }
}
