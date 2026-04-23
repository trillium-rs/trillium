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
//!
//! The module-wide `dead_code` allow is transient — the codec is consumed by the h2
//! connection driver once phase 3 stream machinery lands. Remove once that happens.
#![allow(dead_code)]

pub(crate) mod decoder;
pub(crate) mod dynamic_table;
pub(crate) mod encoder;
pub(crate) mod static_table;

use super::compression_error::CompressionError;
pub(crate) use super::field_section::FieldSection;
use dynamic_table::DynamicTable;

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
    pub(crate) fn decode(
        &mut self,
        block: &[u8],
    ) -> Result<FieldSection<'static>, CompressionError> {
        decoder::decode(block, &mut self.table, self.protocol_max_table_size)
    }
}
