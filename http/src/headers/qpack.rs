//! QPACK types
//!
//! Please note that this interface is likely to change
#[cfg(test)]
mod decoder_corpus_tests;
mod decoder_dynamic_table;
#[cfg(test)]
mod encoder_corpus_tests;
mod encoder_dynamic_table;
mod instruction;
#[cfg(test)]
mod qif;
#[cfg(test)]
mod reference_out;
pub(crate) mod static_table;
#[cfg(test)]
mod tests;

// Wire-format constants for §4.5 field sections live in `instruction::field_section`.
// Encoder-stream instruction constants (§3.2) live in `instruction::encoder`.
// Decoder-stream instruction constants (§4.4) live in `instruction::decoder`.
// §4.1.2 string-literal encoding helpers live in `instruction` (module-level).
//
// Shared with (future) HPACK and re-exported here for the feature-gated qpack public API:
// - `FieldSection`, `PseudoHeaders`, `FieldLineValue` live at `headers::field_section`.
// - `HuffmanError` (RFC 7541 Appendix B codec) lives at `headers::huffman`.
pub use super::field_section::{FieldSection, PseudoHeaders};
#[cfg(feature = "unstable")]
pub use super::huffman::HuffmanError;
pub(crate) use super::{
    field_section::FieldLineValue,
    header_observer::{ConnectionAccumulator, HeaderObserver},
};
#[cfg(not(feature = "unstable"))]
pub(crate) use decoder_dynamic_table::DecoderDynamicTable;
#[cfg(feature = "unstable")]
pub use decoder_dynamic_table::DecoderDynamicTable;
#[cfg(not(feature = "unstable"))]
pub(crate) use encoder_dynamic_table::EncoderDynamicTable;
#[cfg(feature = "unstable")]
pub use encoder_dynamic_table::EncoderDynamicTable;
