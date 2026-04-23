//! HPACK header compression (RFC 7541).
//!
//! HPACK is HTTP/2's header compression format. Compared to QPACK (RFC 9204) it is simpler:
//! inserts into the dynamic table happen inline with the containing header block rather
//! than on a separate encoder stream, so there is no out-of-order delivery, no blocking,
//! no section acknowledgements, and a single [`DynamicTable`] serves both the decoder and
//! the encoder within a connection.
//!
//! This module reuses the shared primitives out of `headers/`:
//! - [`crate::headers::huffman`] — RFC 7541 Appendix B code (identical for HPACK/QPACK).
//! - [`crate::headers::integer_prefix`] — RFC 7541 §5.1 prefix integer codec.
//! - [`crate::headers::entry_name::EntryName`] / `PseudoHeaderName` — shared name vocabulary.
//! - [`crate::headers::field_section::FieldSection`] — decoded output shape.
//! - [`crate::headers::compression_error::CompressionError`] — shared codec error.
pub(crate) mod decoder;
pub(crate) mod dynamic_table;
pub(crate) mod encoder;
pub(crate) mod static_table;

pub use super::field_section::{FieldSection, PseudoHeaders};
use super::{
    compression_error::CompressionError, huffman::HuffmanError, integer_prefix::IntegerPrefixError,
};
pub(crate) use decoder::MalformedRequest;
use dynamic_table::DynamicTable;
#[cfg(feature = "unstable")]
pub use encoder::encode;
#[cfg(not(feature = "unstable"))]
pub(crate) use encoder::encode;

/// Error surfaced out of HPACK decoding.
///
/// The two variants carry different consequences for the driver: [`Compression`] means the
/// dynamic table state is now inconsistent with the peer's model (RFC 7541 §6.3), which
/// forces a connection-level `COMPRESSION_ERROR` GOAWAY. [`MalformedRequest`] is a
/// spec-defined request malformation (§8.1.2) that leaves the dynamic table consistent —
/// the driver emits `RST_STREAM(PROTOCOL_ERROR)` on the offending stream and the connection
/// stays alive.
///
/// [`Compression`]: Self::Compression
/// [`MalformedRequest`]: Self::MalformedRequest
#[derive(Debug, thiserror::Error)]
pub(crate) enum HpackDecodeError {
    /// Wire-format failure — Huffman malformed, integer-prefix decode, oversized size update,
    /// out-of-range table index, truncated input. Connection-level.
    #[error(transparent)]
    Compression(#[from] CompressionError),

    /// Spec-defined request malformation detected during decode (duplicate pseudo,
    /// pseudo-header after a regular header). Stream-level.
    #[error("malformed request header block: {0:?}")]
    MalformedRequest(MalformedRequest),
}

// Transitive `?`-friendly conversions — each of these is covered by
// `From<X> for CompressionError` followed by `From<CompressionError> for HpackDecodeError`,
// but Rust's `?` only walks one `From` step.
impl From<HuffmanError> for HpackDecodeError {
    fn from(e: HuffmanError) -> Self {
        Self::Compression(e.into())
    }
}

impl From<IntegerPrefixError> for HpackDecodeError {
    fn from(e: IntegerPrefixError) -> Self {
        Self::Compression(e.into())
    }
}

/// Per-connection HPACK codec state for the decoder side.
///
/// Wraps the [`DynamicTable`] and the protocol-advertised maximum table size (the value of our
/// outgoing `SETTINGS_HEADER_TABLE_SIZE`, which caps any §6.3 size update the peer can send).
/// Constructed once per HTTP/2 connection and threaded through every header block as the driver
/// task processes them.
#[derive(Debug)]
pub(crate) struct HpackDecoder {
    table: DynamicTable,
    protocol_max_table_size: usize,
}

impl HpackDecoder {
    /// Construct an HPACK decoder. `protocol_max_table_size` is the limit we advertise to the
    /// peer (typically 4096 per RFC 7541's default); §6.3 size updates that exceed it are decoder
    /// errors. The table starts empty at the same size.
    pub(crate) fn new(protocol_max_table_size: usize) -> Self {
        Self {
            table: DynamicTable::new(protocol_max_table_size),
            protocol_max_table_size,
        }
    }

    /// Decode a single complete header block (HEADERS + CONTINUATIONs already reassembled).
    /// Mutates the dynamic table per any incremental-indexing or §6.3 size-update directives in
    /// the block; subsequent blocks see the updated table.
    ///
    /// # Errors
    ///
    /// [`HpackDecodeError::Compression`] for any RFC 7541 wire-format violation
    /// (connection-level `COMPRESSION_ERROR`); [`HpackDecodeError::MalformedRequest`] for
    /// request malformation detected during decode (stream-level `PROTOCOL_ERROR`).
    pub(crate) fn decode(
        &mut self,
        block: &[u8],
    ) -> Result<FieldSection<'static>, HpackDecodeError> {
        decoder::decode(block, &mut self.table, self.protocol_max_table_size)
    }
}
