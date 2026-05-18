//! QPACK header compression (RFC 9204).
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

// Shared with HPACK out of `headers/`:
// - `FieldSection`, `PseudoHeaders`, `FieldLineValue` at `headers::field_section`.
// - `HuffmanError` at `headers::huffman`.
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
